//! A thin, allocation-free wrapper over the Performance Data Helper (PDH) API.
//!
//! The wrapper obeys two rules from the plan:
//! 1. Create each query once. Add counters with `PdhAddEnglishCounterW`, so
//!    the paths work on any system locale.
//! 2. Reuse every buffer, so a steady tick allocates nothing.

use std::fmt;

use windows::core::PCWSTR;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

/// `PDH_FMT_NOCAP100`. The `windows` crate does not bind this flag.
/// Without it PDH clamps a percent counter to 100, and the CPU boost ratio
/// above the base clock disappears.
pub const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

/// Returned when PDH itself fails. The code is a PDH status, not a Win32 error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdhError(pub u32);

impl fmt::Display for PdhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PDH status 0x{:08X}", self.0)
    }
}

impl std::error::Error for PdhError {}

pub type Result<T> = std::result::Result<T, PdhError>;

fn check(status: u32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(PdhError(status))
    }
}

/// Make a NUL-terminated UTF-16 string for a Win32 call.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A PDH query. Every counter it holds collects in one `collect` call.
pub struct Query {
    handle: PDH_HQUERY,
}

impl Query {
    pub fn new() -> Result<Self> {
        let mut handle = PDH_HQUERY::default();
        unsafe { check(PdhOpenQueryW(PCWSTR::null(), 0, &mut handle))? };
        Ok(Self { handle })
    }

    /// Add one counter path. A path may hold a `*` wildcard.
    pub fn add(&self, path: &str) -> Result<Counter> {
        let path = wide(path);
        let mut handle = PDH_HCOUNTER::default();
        unsafe {
            check(PdhAddEnglishCounterW(
                self.handle,
                PCWSTR(path.as_ptr()),
                0,
                &mut handle,
            ))?;
        }
        Ok(Counter {
            handle,
            items: Vec::new(),
            name: String::new(),
        })
    }

    /// Add a counter, but treat "the counter does not exist here" as absence.
    pub fn add_optional(&self, path: &str) -> Option<Counter> {
        self.add(path).ok()
    }

    /// Sample every counter in the query.
    ///
    /// A rate counter needs two collects before it reports a value. The first
    /// collect of a fresh query therefore yields no usable data.
    pub fn collect(&self) -> Result<()> {
        unsafe { check(PdhCollectQueryData(self.handle)) }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.handle);
        }
    }
}

// A PDH handle is a plain kernel handle, and the sampler owns its query alone.
unsafe impl Send for Query {}

/// One counter inside a [`Query`]. It owns the buffers that its reads need.
///
/// A counter has no `Drop`: `PdhCloseQuery` releases every counter in the
/// query, and a later `PdhRemoveCounter` on a closed query is wrong.
pub struct Counter {
    handle: PDH_HCOUNTER,
    /// The scratch block that PDH fills. Its `len` stays 0 always, and its
    /// `capacity` is the buffer size, because PDH writes the records itself.
    items: Vec<PDH_FMT_COUNTERVALUE_ITEM_W>,
    name: String,
}

impl Counter {
    /// Read a single-instance counter as a `f64`.
    pub fn value(&self) -> Result<f64> {
        self.value_with(PDH_FMT(PDH_FMT_DOUBLE.0))
    }

    /// Read a single-instance counter, uncapped at 100.
    pub fn value_nocap(&self) -> Result<f64> {
        self.value_with(PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100))
    }

    /// Grow the scratch block to hold at least `wanted` records.
    ///
    /// `self.items.len()` is always 0, so `Vec::reserve` asks for an absolute
    /// capacity here, never a delta. The `max` keeps the growth monotonic:
    /// PDH reports the size it needs for the instance set it saw, and that
    /// set can grow again before the retry, which would otherwise loop.
    fn grow_to(&mut self, wanted: usize) {
        debug_assert_eq!(self.items.len(), 0);
        let capacity = self.items.capacity();
        let target = wanted.max(capacity + capacity / 2 + 16);
        self.items.reserve(target);
    }

    fn value_with(&self, fmt: PDH_FMT) -> Result<f64> {
        let mut out = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            check(PdhGetFormattedCounterValue(
                self.handle,
                fmt,
                None,
                &mut out,
            ))?;
            Ok(out.Anonymous.doubleValue)
        }
    }

    /// Visit every instance of a wildcard counter.
    ///
    /// The closure gets the instance name and the value. Both borrow buffers
    /// that this counter owns, so a steady tick allocates nothing.
    ///
    /// PDH packs the item array and the instance strings into one buffer, so
    /// the buffer must hold more bytes than `count * size_of::<item>()`. The
    /// code therefore sizes in item units, which also keeps the alignment that
    /// `PDH_FMT_COUNTERVALUE_ITEM_W` needs.
    pub fn for_each_instance(&mut self, mut visit: impl FnMut(&str, f64)) -> Result<()> {
        const ITEM_SIZE: usize = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0);

        for _ in 0..3 {
            let mut bytes = (self.items.capacity() * ITEM_SIZE) as u32;
            let mut count = 0u32;
            let buffer = if self.items.capacity() == 0 {
                None
            } else {
                Some(self.items.as_mut_ptr())
            };
            let status = unsafe {
                PdhGetFormattedCounterArrayW(self.handle, fmt, &mut bytes, &mut count, buffer)
            };

            if status == PDH_MORE_DATA {
                self.grow_to((bytes as usize).div_ceil(ITEM_SIZE));
                continue;
            }
            check(status)?;

            let base = self.items.as_ptr();
            for index in 0..count as usize {
                // PDH wrote `count` items into the reserved block.
                let item = unsafe { *base.add(index) };
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                let text = item.szName.0;
                if text.is_null() {
                    continue;
                }
                self.name.clear();
                unsafe {
                    let mut len = 0usize;
                    while *text.add(len) != 0 {
                        len += 1;
                    }
                    let units = std::slice::from_raw_parts(text, len);
                    for unit in char::decode_utf16(units.iter().copied()) {
                        self.name.push(unit.unwrap_or(char::REPLACEMENT_CHARACTER));
                    }
                }
                visit(&self.name, value);
            }
            return Ok(());
        }
        Err(PdhError(PDH_MORE_DATA))
    }
}
