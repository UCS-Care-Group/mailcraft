// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Graphics instance configuration (Windows: DirectX 12 or OpenGL; Linux and FreeBSD: avoid a
//! software Vulkan), applied before wgpu can initialize any driver, and the check after start-up
//! for an adapter that is a processor.

use eframe::{
    NativeOptions,
    egui_wgpu::WgpuSetup,
    wgpu::{Backends, DeviceType},
};

/// What the start-up probe learned about one adapter (DirectX 12 on Windows, Vulkan and OpenGL
/// elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbedAdapter {
    /// `Cpu` is WARP (Microsoft's software rasteriser): it works, but every frame is drawn on the
    /// processor, which makes the app lag on older machines (#316).
    pub device_type: DeviceType,
    /// Whether the adapter offers the limits eframe will ask for when it creates the device.
    pub meets_limits: bool,
}

/// The backends to initialize on Windows.
///
/// An explicit `backend_override` (normally `Backends::from_env()`, i.e. `WGPU_BACKEND`) always
/// wins. Otherwise DirectX 12 only, as long as `dx12_adapters` (what a DirectX 12-only probe found)
/// contains a usable hardware GPU. Without one, OpenGL is the way to the GPU: graphics chips whose
/// driver has no DirectX 12 support (Intel 4th generation on Windows 11, #316) still have an
/// OpenGL driver, and GL can't reach the AMD driver crash described in [`configure_backends`],
/// because an AMD GPU with a driver shows up as a DirectX 12 adapter and keeps us on DirectX 12.
/// When DirectX 12 offers only WARP, it stays enabled next to GL as a last resort for machines
/// without an OpenGL driver either (virtual machines): wgpu prefers any GPU over a software
/// adapter, so it's only picked when GL has nothing better.
#[cfg(any(target_os = "windows", test))]
pub fn choose_backends(backend_override: Option<Backends>, dx12_adapters: &[ProbedAdapter]) -> Backends {
    if let Some(backends) = backend_override {
        return backends;
    }
    let hardware = |a: &&ProbedAdapter| a.device_type != DeviceType::Cpu;
    if dx12_adapters.iter().filter(hardware).any(|a| a.meets_limits) {
        Backends::DX12
    } else if !dx12_adapters.is_empty() && !dx12_adapters.iter().any(|a| hardware(&a)) {
        Backends::DX12 | Backends::GL
    } else {
        // No DirectX 12 adapter at all, or only GPUs that can't run the app: GL alone, so a
        // DirectX 12 GPU that would fail device creation isn't picked over the OpenGL one.
        Backends::GL
    }
}

/// The backends to initialize on Linux and FreeBSD.
///
/// An explicit `backend_override` (`WGPU_BACKEND`) always wins. A usable hardware Vulkan adapter
/// keeps eframe's default (Vulkan and OpenGL). When Vulkan offers only software adapters
/// (Mesa's lavapipe, when the card's driver is one Mesa can't use) or none: OpenGL alone if it
/// has a usable hardware adapter, so the software Vulkan one can't be picked; else both, so
/// that a machine without any card still starts (wgpu prefers any hardware adapter over a
/// software one).
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub fn choose_unix_backends(backend_override: Option<Backends>, vulkan: &[ProbedAdapter], gl: &[ProbedAdapter]) -> Backends {
    if let Some(backends) = backend_override {
        return backends;
    }
    let usable = |list: &[ProbedAdapter]| list.iter().any(|a| a.device_type != DeviceType::Cpu && a.meets_limits);
    if usable(vulkan) {
        Backends::PRIMARY | Backends::GL
    } else if usable(gl) {
        Backends::GL
    } else {
        Backends::VULKAN | Backends::GL
    }
}

/// Whether the adapter eframe ended up with is a processor (a software rasteriser).
pub fn is_software_adapter(device_type: DeviceType) -> bool {
    device_type == DeviceType::Cpu
}

/// Restricts the wgpu instance eframe creates to `backends`.
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd", test))]
pub fn configure_backends(options: &mut NativeOptions, backends: Backends) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        // eframe's default includes GL. Creating that backend can crash inside AMD's
        // atio6axx.dll before adapter selection or any Rust error handling runs, so the
        // window never appears. Choosing an adapter afterwards is too late: only leaving GL
        // out of the instance avoids it. This matches the verified WGPU_BACKEND=dx12
        // workaround without mutating process environment variables.
        create.instance_descriptor.backends = backends;
    }
}

/// Chooses the backends ([`choose_backends`]) and applies them to `options`. Without a
/// `backend_override` this first probes DirectX 12 with a short-lived instance of its own, which
/// is dropped before eframe creates the real one.
#[cfg(target_os = "windows")]
pub fn configure(options: &mut NativeOptions, backend_override: Option<Backends>) {
    let backends = match backend_override {
        Some(backends) => {
            log::info!("graphics: WGPU_BACKEND override, backends {backends:?}");
            backends
        }
        None => {
            let adapters = probe_dx12(options);
            let backends = choose_backends(None, &adapters);
            log::info!("graphics: {} DirectX 12 adapter(s) {adapters:?}, backends {backends:?}", adapters.len());
            backends
        }
    };
    configure_backends(options, backends);
}

/// Chooses the backends ([`choose_unix_backends`]) and applies them to `options`; without a
/// `backend_override` this first probes Vulkan and OpenGL, each with a short-lived instance.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn configure(options: &mut NativeOptions, backend_override: Option<Backends>) {
    let backends = match backend_override {
        Some(backends) => {
            log::info!("graphics: WGPU_BACKEND override, backends {backends:?}");
            backends
        }
        None => {
            let vulkan = probe(options, Backends::VULKAN);
            let gl =
                if vulkan.iter().any(|a| a.device_type != DeviceType::Cpu && a.meets_limits) { Vec::new() } else { probe(options, Backends::GL) };
            let backends = choose_unix_backends(None, &vulkan, &gl);
            log::info!("graphics: {} Vulkan adapter(s) {vulkan:?}, {} OpenGL adapter(s) {gl:?}, backends {backends:?}", vulkan.len(), gl.len());
            backends
        }
    };
    configure_backends(options, backends);
}

/// Lists the adapters of one backend, checking each against the limits eframe's device
/// descriptor will require. Empty when this build has no such backend.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn probe(options: &NativeOptions, backend: Backends) -> Vec<ProbedAdapter> {
    use eframe::wgpu::{Instance, InstanceDescriptor};
    if !Instance::enabled_backend_features().contains(backend) {
        return Vec::new();
    }
    let WgpuSetup::CreateNew(create) = &options.wgpu_options.wgpu_setup else {
        return Vec::new();
    };
    let instance = Instance::new(InstanceDescriptor { backends: backend, ..InstanceDescriptor::new_without_display_handle() });
    crate::block_on::block_on(instance.enumerate_adapters(backend))
        .iter()
        .map(|adapter| {
            let info = adapter.get_info();
            let required = (create.device_descriptor)(adapter).required_limits;
            let meets_limits = required.check_limits(&adapter.limits());
            log::info!("graphics: {backend:?} adapter {:?} ({:?}, driver {:?}), limits ok: {meets_limits}", info.name, info.device_type, info.driver);
            ProbedAdapter { device_type: info.device_type, meets_limits }
        })
        .collect()
}

/// Lists the DirectX 12 adapters, checking each against the limits eframe's device descriptor
/// will require. Empty when this build has no DirectX 12 backend.
#[cfg(target_os = "windows")]
fn probe_dx12(options: &NativeOptions) -> Vec<ProbedAdapter> {
    use eframe::wgpu::{Instance, InstanceDescriptor};
    // `Instance::new` panics when the build has no backend for the platform.
    if !Instance::enabled_backend_features().contains(Backends::DX12) {
        return Vec::new();
    }
    let WgpuSetup::CreateNew(create) = &options.wgpu_options.wgpu_setup else {
        return Vec::new();
    };
    let instance = Instance::new(InstanceDescriptor { backends: Backends::DX12, ..InstanceDescriptor::new_without_display_handle() });
    crate::block_on::block_on(instance.enumerate_adapters(Backends::DX12))
        .iter()
        .map(|adapter| {
            let info = adapter.get_info();
            let required = (create.device_descriptor)(adapter).required_limits;
            let meets_limits = required.check_limits(&adapter.limits());
            log::info!("graphics: DirectX 12 adapter {:?} ({:?}, driver {:?}), limits ok: {meets_limits}", info.name, info.device_type, info.driver);
            ProbedAdapter { device_type: info.device_type, meets_limits }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_backends(backends: Backends) -> Backends {
        let mut options = NativeOptions::default();
        configure_backends(&mut options, backends);
        let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        create.instance_descriptor.backends
    }

    const fn adapter(device_type: DeviceType, meets_limits: bool) -> ProbedAdapter {
        ProbedAdapter { device_type, meets_limits }
    }

    #[test]
    fn windows_default_initializes_only_dx12() {
        // eframe's own default (PRIMARY | GL) would initialize OpenGL as well.
        let WgpuSetup::CreateNew(eframe_default) = NativeOptions::default().wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        assert!(eframe_default.instance_descriptor.backends.contains(Backends::GL));
        let gpu = [adapter(DeviceType::IntegratedGpu, true)];
        assert_eq!(configured_backends(choose_backends(None, &gpu)), Backends::DX12);
    }

    #[test]
    fn explicit_backend_override_is_preserved() {
        let warp_only = [adapter(DeviceType::Cpu, true)];
        for backends in [Backends::VULKAN, Backends::GL, Backends::DX12, Backends::VULKAN | Backends::DX12, Backends::empty()] {
            // Whatever the probe would have found, the override wins (the probe doesn't run then).
            assert_eq!(choose_backends(Some(backends), &[]), backends);
            assert_eq!(choose_backends(Some(backends), &warp_only), backends);
            assert_eq!(configured_backends(backends), backends);
        }
    }

    #[test]
    fn gl_when_dx12_has_no_usable_gpu() {
        // #316: no DirectX 12 driver, so DirectX 12 offers only WARP (or nothing) and lags.
        let warp_only = [adapter(DeviceType::Cpu, true)];
        assert_eq!(choose_backends(None, &warp_only), Backends::GL | Backends::DX12);
        assert_eq!(choose_backends(None, &[]), Backends::GL);
        // A GPU that can't create eframe's device: GL alone, so it isn't picked over the GL one.
        let weak_gpu = [adapter(DeviceType::IntegratedGpu, false), adapter(DeviceType::Cpu, true)];
        assert_eq!(choose_backends(None, &weak_gpu), Backends::GL);
        // Any usable GPU keeps DirectX 12 only, next to WARP or a weaker GPU.
        let mixed = [adapter(DeviceType::Cpu, true), adapter(DeviceType::IntegratedGpu, false), adapter(DeviceType::DiscreteGpu, true)];
        assert_eq!(choose_backends(None, &mixed), Backends::DX12);
    }

    #[test]
    fn unix_override_is_preserved_and_hardware_vulkan_keeps_the_default() {
        let cpu = [adapter(DeviceType::Cpu, true)];
        for backends in [Backends::VULKAN, Backends::GL, Backends::empty()] {
            assert_eq!(choose_unix_backends(Some(backends), &cpu, &[]), backends);
        }
        let gpu = [adapter(DeviceType::DiscreteGpu, true), adapter(DeviceType::Cpu, true)];
        assert_eq!(choose_unix_backends(None, &gpu, &[]), Backends::PRIMARY | Backends::GL);
    }

    #[test]
    fn unix_software_vulkan_moves_to_hardware_gl() {
        // The report: Mesa can't use the card's driver, so Vulkan shows only lavapipe.
        let lavapipe = [adapter(DeviceType::Cpu, true)];
        let gl_gpu = [adapter(DeviceType::DiscreteGpu, true)];
        assert_eq!(choose_unix_backends(None, &lavapipe, &gl_gpu), Backends::GL);
        assert_eq!(choose_unix_backends(None, &[], &gl_gpu), Backends::GL);
        // A hardware GL adapter that can't make the device doesn't count.
        let weak = [adapter(DeviceType::IntegratedGpu, false)];
        assert_eq!(choose_unix_backends(None, &lavapipe, &weak), Backends::VULKAN | Backends::GL);
        // Nothing but software: both stay, the machine still starts.
        assert_eq!(choose_unix_backends(None, &lavapipe, &[adapter(DeviceType::Cpu, true)]), Backends::VULKAN | Backends::GL);
        assert_eq!(choose_unix_backends(None, &[], &[]), Backends::VULKAN | Backends::GL);
    }

    #[test]
    fn only_a_processor_is_software() {
        assert!(is_software_adapter(DeviceType::Cpu));
        for t in [DeviceType::DiscreteGpu, DeviceType::IntegratedGpu, DeviceType::VirtualGpu, DeviceType::Other] {
            assert!(!is_software_adapter(t));
        }
    }
}
