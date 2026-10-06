//! Adapter discovery through the WDDM kernel-mode thunk (`D3DKMT`).
//!
//! DXCore misses an NPU whose driver does not expose a Direct3D 12 device.
//! Spike 1 proved that: the AMD XDNA NPU in this machine publishes
//! `GPU Engine` counters, but DXCore does not list it at all.
//!
//! WDDM sees every adapter. `KMTQAITYPE_ADAPTERTYPE` carries a `ComputeOnly`
//! flag, which is the definitive NPU signal, and
//! `KMTQAITYPE_ADAPTERREGISTRYINFO` carries the friendly name.

use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTEnumAdapters3, D3DKMTQueryAdapterInfo,
    D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_ADAPTERTYPE, D3DKMT_CLOSEADAPTER,
    D3DKMT_ENUMADAPTERS2, D3DKMT_ENUMADAPTERS3, D3DKMT_ENUMADAPTERS_FILTER,
    D3DKMT_QUERYADAPTERINFO, KMTQAITYPE_ADAPTERREGISTRYINFO, KMTQAITYPE_ADAPTERTYPE,
};

use crate::luid::AdapterLuid;

/// `STATUS_BUFFER_TOO_SMALL`. An enumeration returns it when the adapter
/// count grew between the sizing call and the read call.
const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;

/// `IncludeComputeOnly | IncludeDisplayOnly | IncludeVirtualGpuOnly`.
///
/// The default enumeration drops a compute-only adapter "to avoid breaking
/// applications", so `D3DKMTEnumAdapters2` never reports an NPU. Spike 1
/// confirmed that on this machine. Only `D3DKMTEnumAdapters3` with these
/// filter bits reports it.
const FILTER_INCLUDE_ALL: u64 = 0b111;

/// Bit positions inside `D3DKMT_ADAPTERTYPE`. The header
/// `shared/d3dkmthk.h` fixes this order.
const BIT_RENDER_SUPPORTED: u32 = 0;
const BIT_DISPLAY_SUPPORTED: u32 = 1;
const BIT_SOFTWARE_DEVICE: u32 = 2;
const BIT_COMPUTE_ONLY: u32 = 11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WddmAdapter {
    pub luid: AdapterLuid,
    /// The driver name, such as `AMD Radeon(TM) 890M Graphics`.
    pub name: String,
    /// The adapter computes but never displays. An NPU sets this flag.
    pub compute_only: bool,
    pub render_supported: bool,
    pub display_supported: bool,
    pub software_device: bool,
}

impl WddmAdapter {
    /// True when this adapter is an NPU: real hardware that only computes.
    pub fn is_npu(&self) -> bool {
        self.compute_only && !self.software_device
    }
}

fn bit(field: u32, position: u32) -> bool {
    field & (1 << position) != 0
}

/// Read one `KMTQUERYADAPTERINFOTYPE` record for an open adapter handle.
fn query<T: Default>(
    handle: u32,
    kind: windows::Wdk::Graphics::Direct3D::KMTQUERYADAPTERINFOTYPE,
) -> Option<T> {
    let mut out = T::default();
    let mut request = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: kind,
        pPrivateDriverData: (&raw mut out).cast::<core::ffi::c_void>(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    let status = unsafe { D3DKMTQueryAdapterInfo(&raw mut request) };
    if status.0 == 0 {
        Some(out)
    } else {
        None
    }
}

fn name_of(handle: u32) -> String {
    let Some(info) = query::<D3DKMT_ADAPTERREGISTRYINFO>(handle, KMTQAITYPE_ADAPTERREGISTRYINFO)
    else {
        return String::new();
    };
    let units = &info.AdapterString;
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end]).trim().to_string()
}

/// Count the adapters, then read them. `buffer` ends up holding `count`
/// records. A hot-plug between the two calls changes the count, so retry.
fn list_adapters(
    size: impl Fn() -> Option<u32>,
    read: impl Fn(&mut [D3DKMT_ADAPTERINFO]) -> Result<u32, i32>,
    buffer: &mut Vec<D3DKMT_ADAPTERINFO>,
) -> u32 {
    for _ in 0..3 {
        let Some(wanted) = size() else { return 0 };
        buffer.clear();
        buffer.resize(wanted as usize, D3DKMT_ADAPTERINFO::default());
        if buffer.is_empty() {
            return 0;
        }
        match read(buffer) {
            Ok(got) => return got.min(wanted),
            Err(STATUS_BUFFER_TOO_SMALL) => continue,
            Err(_) => return 0,
        }
    }
    0
}

/// Enumerate every WDDM adapter, display and compute alike.
pub fn enumerate() -> Vec<WddmAdapter> {
    let mut adapters = Vec::new();
    let mut raw: Vec<D3DKMT_ADAPTERINFO> = Vec::new();

    let filter = D3DKMT_ENUMADAPTERS_FILTER {
        Value: FILTER_INCLUDE_ALL,
    };
    let mut count = list_adapters(
        || {
            let mut request = D3DKMT_ENUMADAPTERS3 {
                Filter: filter,
                NumAdapters: 0,
                pAdapters: std::ptr::null_mut(),
            };
            let status = unsafe { D3DKMTEnumAdapters3(&raw mut request) };
            (status.0 == 0).then_some(request.NumAdapters)
        },
        |buffer| {
            let mut request = D3DKMT_ENUMADAPTERS3 {
                Filter: filter,
                NumAdapters: buffer.len() as u32,
                pAdapters: buffer.as_mut_ptr(),
            };
            let status = unsafe { D3DKMTEnumAdapters3(&raw mut request) };
            if status.0 == 0 {
                Ok(request.NumAdapters)
            } else {
                Err(status.0)
            }
        },
        &mut raw,
    );

    // `D3DKMTEnumAdapters3` needs WDDM 2.9. Fall back on an older system,
    // and accept that the NPU row is then absent (GLINT-NPU-OPTIONAL).
    if count == 0 {
        count = list_adapters(
            || {
                let mut request = D3DKMT_ENUMADAPTERS2 {
                    NumAdapters: 0,
                    pAdapters: std::ptr::null_mut(),
                };
                let status = unsafe { D3DKMTEnumAdapters2(&raw mut request) };
                (status.0 == 0).then_some(request.NumAdapters)
            },
            |buffer| {
                let mut request = D3DKMT_ENUMADAPTERS2 {
                    NumAdapters: buffer.len() as u32,
                    pAdapters: buffer.as_mut_ptr(),
                };
                let status = unsafe { D3DKMTEnumAdapters2(&raw mut request) };
                if status.0 == 0 {
                    Ok(request.NumAdapters)
                } else {
                    Err(status.0)
                }
            },
            &mut raw,
        );
    }

    for info in raw.iter().take(count as usize) {
        let kind = query::<D3DKMT_ADAPTERTYPE>(info.hAdapter, KMTQAITYPE_ADAPTERTYPE);
        let flags = kind
            .map(|kind| unsafe { kind.Anonymous.Anonymous._bitfield })
            .unwrap_or(0);
        adapters.push(WddmAdapter {
            luid: AdapterLuid {
                high: info.AdapterLuid.HighPart as u32,
                low: info.AdapterLuid.LowPart,
            },
            name: name_of(info.hAdapter),
            compute_only: bit(flags, BIT_COMPUTE_ONLY),
            render_supported: bit(flags, BIT_RENDER_SUPPORTED),
            display_supported: bit(flags, BIT_DISPLAY_SUPPORTED),
            software_device: bit(flags, BIT_SOFTWARE_DEVICE),
        });

        // Every handle from the enumeration needs a close.
        let close = D3DKMT_CLOSEADAPTER {
            hAdapter: info.hAdapter,
        };
        unsafe {
            let _ = D3DKMTCloseAdapter(&raw const close);
        }
    }
    adapters
}

/// Pick the NPU, if the machine has one.
pub fn npu(adapters: &[WddmAdapter]) -> Option<&WddmAdapter> {
    adapters.iter().find(|a| a.is_npu())
}

/// Pick the adapter that the GPU row should show: real hardware that renders.
/// A discrete adapter beats an integrated one, and both beat a software one.
pub fn primary_gpu(adapters: &[WddmAdapter]) -> Option<&WddmAdapter> {
    adapters
        .iter()
        .filter(|a| a.render_supported && !a.compute_only)
        .max_by_key(|a| (!a.software_device, a.display_supported))
}
