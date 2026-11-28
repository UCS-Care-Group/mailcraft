// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows graphics instance configuration, applied before wgpu can initialize any driver.

use eframe::{
    NativeOptions,
    egui_wgpu::WgpuSetup,
    wgpu::{Backends, DeviceType, Dx12SwapchainKind},
};

/// What the start-up probe learned about one DirectX 12 adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dx12Adapter {
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
pub fn choose_backends(backend_override: Option<Backends>, dx12_adapters: &[Dx12Adapter]) -> Backends {
    if let Some(backends) = backend_override {
        return backends;
    }
    let hardware = |a: &&Dx12Adapter| a.device_type != DeviceType::Cpu;
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

/// Restricts the wgpu instance eframe creates to `backends`.
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

/// How DirectX 12 hands the window's frames to the desktop.
///
/// An explicit `presentation_override` (normally `Dx12SwapchainKind::from_env()`, i.e.
/// `WGPU_DX12_PRESENTATION_SYSTEM`) always wins. Otherwise through a DirectComposition visual over
/// the window, as browsers and the system's own programs present, so that the window can be
/// composed with premultiplied alpha ([`premultiplied_backbuffer`]).
///
/// Why: a swap chain made straight from the window is promoted by Windows to independent flip as
/// soon as the window fills the screen (maximized, the owner's usual way): it then takes a plane
/// of the display of its own, as a full-screen game does. NVIDIA's overlay watches the first
/// frames a program presents, and one presented that way makes it take the program for a game:
/// it puts up its "Alt+Z" notice and offers to record and stream MailCraft. Its own log
/// (`ShadowPlay\CaptureCore.log`) then says `game[1]` and `bIFlip = 1`; otherwise "Not a game".
pub fn choose_presentation(presentation_override: Option<Dx12SwapchainKind>) -> Dx12SwapchainKind {
    presentation_override.unwrap_or(Dx12SwapchainKind::DxgiFromVisual)
}

/// Whether the window's frames go to the desktop with premultiplied alpha: when DirectX 12 alone
/// draws (`backends`) and presents through DirectComposition.
///
/// A DirectComposition swap chain without alpha is still promoted to independent flip, only a
/// little later, and NVIDIA's overlay then catches it some of the time (two maximized starts in
/// six, against every one through the window's own swap chain). With premultiplied alpha the
/// desktop has to blend it itself, every frame, so it isn't promoted (no start in ten, three of
/// them with the window kept repainting through the overlay's first checks). The frames stay opaque: eframe's
/// clear colour is replaced by an opaque one (`App::clear_color` in `main.rs`) and the panels are
/// opaque, so nothing behind the window shows through. eframe gives the swap chain that alpha
/// when the window is marked transparent; on Windows winit does nothing more for it than an empty
/// blur-behind region (no layered window, clicks still reach it). Not when OpenGL is among the
/// backends (no usable DirectX 12 GPU): that path keeps the opaque window it has always had.
pub fn premultiplied_backbuffer(backends: Backends, presentation: Dx12SwapchainKind) -> bool {
    backends == Backends::DX12 && presentation == Dx12SwapchainKind::DxgiFromVisual
}

/// Makes the DirectX 12 surfaces of the wgpu instance eframe creates present through
/// `presentation`, with premultiplied alpha where [`premultiplied_backbuffer`] says so for
/// `backends`. It has no effect on the other backends.
pub fn configure_presentation(options: &mut NativeOptions, backends: Backends, presentation: Dx12SwapchainKind) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.backend_options.dx12.presentation_system = presentation;
    }
    if premultiplied_backbuffer(backends, presentation) {
        options.viewport.transparent = Some(true);
    }
}

/// Chooses the backends ([`choose_backends`]) and the DirectX 12 presentation
/// ([`choose_presentation`]) and applies them to `options`. Without a `backend_override` this
/// first probes DirectX 12 with a short-lived instance of its own, which is dropped before eframe
/// creates the real one.
#[cfg(target_os = "windows")]
pub fn configure(options: &mut NativeOptions, backend_override: Option<Backends>, presentation_override: Option<Dx12SwapchainKind>) {
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
    let presentation = choose_presentation(presentation_override);
    let premultiplied = premultiplied_backbuffer(backends, presentation);
    log::info!("graphics: DirectX 12 presents through {presentation:?}, premultiplied alpha: {premultiplied}");
    configure_presentation(options, backends, presentation);
}

/// Lists the DirectX 12 adapters, checking each against the limits eframe's device descriptor
/// will require. Empty when this build has no DirectX 12 backend.
#[cfg(target_os = "windows")]
fn probe_dx12(options: &NativeOptions) -> Vec<Dx12Adapter> {
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
            Dx12Adapter { device_type: info.device_type, meets_limits }
        })
        .collect()
}

#[cfg(test)]
#[path = "graphics_tests.rs"]
mod graphics_tests;
