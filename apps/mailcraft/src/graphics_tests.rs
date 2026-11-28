// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The graphics instance eframe is given on Windows: its backends and its DirectX 12 presentation.

use super::*;

fn configured_backends(backends: Backends) -> Backends {
    let mut options = NativeOptions::default();
    configure_backends(&mut options, backends);
    let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
        panic!("default native options must create a wgpu instance");
    };
    create.instance_descriptor.backends
}

const fn adapter(device_type: DeviceType, meets_limits: bool) -> Dx12Adapter {
    Dx12Adapter { device_type, meets_limits }
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

fn configured(backends: Backends, presentation: Dx12SwapchainKind) -> (Dx12SwapchainKind, Option<bool>, Backends) {
    let mut options = NativeOptions::default();
    configure_backends(&mut options, backends);
    configure_presentation(&mut options, backends, presentation);
    let transparent = options.viewport.transparent;
    let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
        panic!("default native options must create a wgpu instance");
    };
    (create.instance_descriptor.backend_options.dx12.presentation_system, transparent, create.instance_descriptor.backends)
}

#[test]
fn dx12_presents_through_directcomposition_with_premultiplied_alpha_by_default() {
    // A swap chain made from the window is promoted to independent flip once the window is
    // maximized, and NVIDIA's overlay then takes MailCraft for a full-screen game; so is one made
    // through DirectComposition without alpha, some of the time.
    let presentation = choose_presentation(None);
    assert_eq!(presentation, Dx12SwapchainKind::DxgiFromVisual);
    assert!(premultiplied_backbuffer(Backends::DX12, presentation));
    // eframe gives the swap chain premultiplied alpha for a transparent window.
    assert_eq!(configured(Backends::DX12, presentation), (Dx12SwapchainKind::DxgiFromVisual, Some(true), Backends::DX12));
}

#[test]
fn explicit_presentation_override_is_preserved() {
    // `WGPU_DX12_PRESENTATION_SYSTEM=hwnd` gives the window's own swap chain back (RenderDoc can
    // capture only that one), opaque as it always was: it can't take alpha.
    assert_eq!(choose_presentation(Some(Dx12SwapchainKind::DxgiFromHwnd)), Dx12SwapchainKind::DxgiFromHwnd);
    assert_eq!(choose_presentation(Some(Dx12SwapchainKind::DxgiFromVisual)), Dx12SwapchainKind::DxgiFromVisual);
    assert!(!premultiplied_backbuffer(Backends::DX12, Dx12SwapchainKind::DxgiFromHwnd));
    assert_eq!(configured(Backends::DX12, Dx12SwapchainKind::DxgiFromHwnd), (Dx12SwapchainKind::DxgiFromHwnd, None, Backends::DX12));
}

#[test]
fn opengl_keeps_an_opaque_window() {
    // #316's machines draw with OpenGL (next to WARP or alone): nothing changes for them, and an
    // override that brings in another backend leaves the window opaque too.
    for backends in [Backends::GL, Backends::GL | Backends::DX12, Backends::VULKAN, Backends::VULKAN | Backends::DX12] {
        assert!(!premultiplied_backbuffer(backends, Dx12SwapchainKind::DxgiFromVisual), "{backends:?}");
        let (_, transparent, configured_backends) = configured(backends, Dx12SwapchainKind::DxgiFromVisual);
        assert_eq!(transparent, None, "{backends:?}");
        assert_eq!(configured_backends, backends);
    }
}

#[test]
fn the_clear_colour_is_opaque() {
    // With premultiplied alpha a see-through clear colour would show what is behind the window
    // wherever nothing is drawn.
    assert_eq!(crate::CLEAR_COLOR.a(), 255);
    assert_eq!(crate::CLEAR_COLOR.to_normalized_gamma_f32()[3], 1.0);
}
