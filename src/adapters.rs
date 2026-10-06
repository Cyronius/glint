//! Adapter discovery through DXCore.
//!
//! The GPU row and the NPU row both read `GPU Engine` counters. Only the
//! adapter LUID tells them apart, so the app must learn which LUID belongs to
//! the NPU. DXCore answers that: an NPU reports the generic machine-learning
//! attribute and does not report the Direct3D 12 graphics attribute.

use windows::core::GUID;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::DXCore::{
    DXCoreAdapterProperty, DXCoreCreateAdapterFactory, DriverDescription, IDXCoreAdapter,
    IDXCoreAdapterFactory, IDXCoreAdapterList, InstanceLuid, IsHardware, IsIntegrated,
    DXCORE_ADAPTER_ATTRIBUTE_D3D12_CORE_COMPUTE, DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS,
};

use crate::luid::AdapterLuid;

/// `DXCORE_ADAPTER_ATTRIBUTE_D3D12_GENERIC_ML`. The `windows` crate does not
/// bind this attribute. An NPU reports it; a plain graphics adapter does not.
pub const ATTRIBUTE_GENERIC_ML: GUID = GUID::from_u128(0xb71b0d41_1088_422f_a27c_0250b7d3a988);

/// What an adapter is for. The display row and the NPU row pick by this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    /// Draws. It carries the Direct3D 12 graphics attribute.
    Graphics,
    /// Computes only. This is the NPU case.
    ComputeOnly,
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub luid: AdapterLuid,
    pub description: String,
    pub kind: AdapterKind,
    pub is_hardware: bool,
    pub is_integrated: bool,
}

/// Read one fixed-size property of an adapter.
fn property<T: Copy + Default>(adapter: &IDXCoreAdapter, which: DXCoreAdapterProperty) -> Option<T> {
    let mut out = T::default();
    let read = unsafe {
        adapter.GetProperty(
            which,
            size_of::<T>(),
            (&raw mut out).cast::<core::ffi::c_void>(),
        )
    };
    read.ok()?;
    Some(out)
}

/// Read the adapter description. DXCore reports it as a NUL-terminated byte
/// string, so the reported length includes the terminator.
fn description(adapter: &IDXCoreAdapter) -> String {
    let size = match unsafe { adapter.GetPropertySize(DriverDescription) } {
        Ok(size) if size > 0 => size,
        _ => return String::new(),
    };
    let mut bytes = vec![0u8; size];
    let read = unsafe {
        adapter.GetProperty(
            DriverDescription,
            size,
            bytes.as_mut_ptr().cast::<core::ffi::c_void>(),
        )
    };
    if read.is_err() {
        return String::new();
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn to_adapter_luid(luid: LUID) -> AdapterLuid {
    AdapterLuid {
        high: luid.HighPart as u32,
        low: luid.LowPart,
    }
}

/// Walk one adapter list and push what it holds. A LUID already in `out`
/// keeps its first classification.
fn push_list(list: &IDXCoreAdapterList, kind: AdapterKind, out: &mut Vec<Adapter>) {
    let count = unsafe { list.GetAdapterCount() };
    for index in 0..count {
        let adapter = match unsafe { list.GetAdapter::<IDXCoreAdapter>(index) } {
            Ok(adapter) => adapter,
            Err(_) => continue,
        };
        let luid = match property::<LUID>(&adapter, InstanceLuid) {
            Some(luid) => to_adapter_luid(luid),
            None => continue,
        };
        if out.iter().any(|a| a.luid == luid) {
            continue;
        }
        out.push(Adapter {
            luid,
            description: description(&adapter),
            kind,
            is_hardware: property::<u8>(&adapter, IsHardware).unwrap_or(0) != 0,
            is_integrated: property::<u8>(&adapter, IsIntegrated).unwrap_or(0) != 0,
        });
    }
}

/// Enumerate every adapter that DXCore reports.
///
/// Graphics adapters come first, so an adapter that carries both the graphics
/// attribute and a compute attribute is classified as graphics, not as an NPU.
pub fn enumerate() -> windows::core::Result<Vec<Adapter>> {
    let mut out = Vec::new();
    let factory: IDXCoreAdapterFactory = unsafe { DXCoreCreateAdapterFactory() }?;

    let graphics: IDXCoreAdapterList =
        unsafe { factory.CreateAdapterList(&[DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS]) }?;
    push_list(&graphics, AdapterKind::Graphics, &mut out);

    // An NPU answers to the machine-learning attribute. An older driver
    // answers only to the core-compute attribute, so try both.
    for attribute in [
        ATTRIBUTE_GENERIC_ML,
        DXCORE_ADAPTER_ATTRIBUTE_D3D12_CORE_COMPUTE,
    ] {
        if let Ok(list) = unsafe { factory.CreateAdapterList::<IDXCoreAdapterList>(&[attribute]) } {
            push_list(&list, AdapterKind::ComputeOnly, &mut out);
        }
    }
    Ok(out)
}

/// Pick the adapter that the GPU row should show.
/// A hardware adapter wins over a software one, and a discrete one over an
/// integrated one.
pub fn primary_gpu(adapters: &[Adapter]) -> Option<&Adapter> {
    adapters
        .iter()
        .filter(|a| a.kind == AdapterKind::Graphics)
        .max_by_key(|a| (a.is_hardware, !a.is_integrated))
}

/// Pick the NPU, if the machine has one.
pub fn npu(adapters: &[Adapter]) -> Option<&Adapter> {
    adapters
        .iter()
        .find(|a| a.kind == AdapterKind::ComputeOnly && a.is_hardware)
}
