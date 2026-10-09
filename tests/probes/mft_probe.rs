//! probe-group: fast
//! VideoProcessorMFT + D3D11 设备管理器的配置组合探针。
//!
//! 背景：给 VPMF 发 `MFT_MESSAGE_SET_D3D_MANAGER` 之后，`ProcessOutput` 一律返回
//! `Output Sample is Invalid (0x80070057)`。要弄清到底哪个环节不满足要求，逐一试组合
//! 比"改一次应用、构建 45 秒、录一段"快得多。
//!
//! 组合维度：
//!   - `SET_D3D_MANAGER` 在设媒体类型**之前**还是**之后**
//!   - 输出样本由调用方提供 / 让 MFT 自己分配
//!   - 输入纹理：带 `GDI_COMPATIBLE` 的采集纹理 / 干净纹理
//!
//! # 2026-09-19 结论（6 种组合全部 OK）
//!
//! ```text
//! 基线：不设 mgr，外部样本，采集纹理        -> OK(buffers=1, flags=0x0,   provides=false)
//! 设 mgr(类型后)，外部样本，采集纹理        -> OK(buffers=1, flags=0x100, provides=true)
//! 设 mgr(类型后)，外部样本，干净纹理        -> OK(buffers=1, flags=0x100, provides=true)
//! 设 mgr(类型后)，MFT 提供样本             -> OK(buffers=1, flags=0x100, provides=true)
//! 设 mgr(类型前)，外部样本，采集纹理        -> OK(buffers=1, flags=0x100, provides=true)
//! 设 mgr(类型前)，MFT 提供样本             -> OK(buffers=1, flags=0x100, provides=true)
//! ```
//!
//! 关键就一条：**设了设备管理器，`GetOutputStreamInfo` 会置上
//! `MFT_OUTPUT_STREAM_PROVIDES_SAMPLES`（0x100），输出样本必须让 MFT 自己分配**
//! （`MFT_OUTPUT_DATA_BUFFER::pSample = None`）。它偏偏**不报错**，只是下一帧
//! `ProcessOutput` 返 `E_INVALIDARG` —— 第一次往主通路里塞这条线时，
//! `recorder.rs` 一边设了 mgr 一边继续自己 `sample_from_texture(&nv12)`，于是每帧都失败、
//! 产物 0 字节。`SET_D3D_MANAGER` 放在类型前/后都行。
//!
//! 跑法：`cargo test --release --test mft_probe -- --ignored --nocapture`

#![cfg(windows)]

use windows::core::{IUnknown, Interface, GUID};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

const W: u32 = 640;
const H: u32 = 360;

unsafe fn make_device() -> (ID3D11Device, ID3D11DeviceContext) {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut dev: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&levels),
        D3D11_SDK_VERSION,
        Some(&mut dev),
        None,
        Some(&mut ctx),
    )
    .unwrap();
    (dev.unwrap(), ctx.unwrap())
}

unsafe fn make_tex(dev: &ID3D11Device, fmt: DXGI_FORMAT, bind: u32, misc: u32) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: misc,
    };
    let mut t: Option<ID3D11Texture2D> = None;
    dev.CreateTexture2D(&desc, None, Some(&mut t)).unwrap();
    t.unwrap()
}

unsafe fn sample_of(tex: &ID3D11Texture2D) -> IMFSample {
    let surf: IDXGISurface = tex.cast().unwrap();
    let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &surf, 0, false).unwrap();
    let s = MFCreateSample().unwrap();
    s.AddBuffer(&buf).unwrap();
    s
}

unsafe fn make_mgr(dev: &ID3D11Device) -> std::result::Result<IMFDXGIDeviceManager, String> {
    let mut token = 0u32;
    let mut mgr: Option<IMFDXGIDeviceManager> = None;
    MFCreateDXGIDeviceManager(&mut token, &mut mgr)
        .map_err(|e| format!("MFCreateDXGIDeviceManager: {e}"))?;
    let mgr = mgr.ok_or_else(|| "MFCreateDXGIDeviceManager 无返回".to_string())?;
    let unk: IUnknown = dev.cast().map_err(|e| format!("device→IUnknown: {e}"))?;
    mgr.ResetDevice(&unk, token)
        .map_err(|e| format!("ResetDevice: {e}"))?;
    Ok(mgr)
}

fn set_type(mft: &IMFTransform, input: bool, sub: &GUID, w: u32, h: u32, stride: i32) -> String {
    unsafe {
        let t = MFCreateMediaType().unwrap();
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
        t.SetGUID(&MF_MT_SUBTYPE, sub).unwrap();
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)
            .unwrap();
        t.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1).unwrap();
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
            .unwrap();
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .unwrap();
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride as u32).unwrap();
        let r = if input {
            mft.SetInputType(0, &t, 0)
        } else {
            mft.SetOutputType(0, &t, 0)
        };
        match r {
            Ok(()) => "ok".into(),
            Err(e) => format!("{e}"),
        }
    }
}

struct Case {
    name: String,
    mgr_before_types: bool,
    use_mgr: bool,
    caller_sample: bool,
    clean_input: bool,
}

unsafe fn run_case(c: &Case) -> String {
    let (dev, ctx) = make_device();
    let mgr = if c.use_mgr {
        match make_mgr(&dev) {
            Ok(m) => Some(m),
            Err(e) => return format!("建设备管理器失败: {e}"),
        }
    } else {
        None
    };
    let mft: IMFTransform =
        CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER).unwrap();

    if c.mgr_before_types {
        if let Some(m) = mgr.as_ref() {
            if let Err(e) = mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize) {
                return format!("SET_D3D_MANAGER(前) 失败: {e}");
            }
        }
    }

    let a = set_type(&mft, true, &MFVideoFormat_ARGB32, W, H, (W * 4) as i32);
    let b = set_type(&mft, false, &MFVideoFormat_NV12, W, H, W as i32);
    if a != "ok" || b != "ok" {
        return format!("设类型失败: in={a} out={b}");
    }

    if !c.mgr_before_types {
        if let Some(m) = mgr.as_ref() {
            if let Err(e) = mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize) {
                return format!("SET_D3D_MANAGER(后) 失败: {e}");
            }
        }
    }

    // 流信息：判断输出样本该由谁提供。
    let info = mft.GetOutputStreamInfo(0).unwrap_or_default();
    let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;

    let _ = mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
    let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
    let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);

    // 输入纹理：带 GDI_COMPATIBLE 的采集纹理 或 干净纹理（都当 ARGB32 用）。
    //
    // ★ bind 两个都要带：`D3D11_RESOURCE_MISC_GDI_COMPATIBLE` 要求纹理同时是
    //   `RENDER_TARGET`（GDI 通过它拿到 DC 去画），只给 `SHADER_RESOURCE` 时
    //   `CreateTexture2D` 直接返回 `E_INVALIDARG`（0x80070057）—— 实测本探针第一版
    //   就崩在这儿，和 `recorder.rs` 里 `acq` 的 bind 组合一致才是对的。
    let in_bind = (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32;
    let in_tex = if c.clean_input {
        make_tex(&dev, DXGI_FORMAT_B8G8R8A8_UNORM, in_bind, 0)
    } else {
        make_tex(
            &dev,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            in_bind,
            D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
        )
    };
    let out_tex = make_tex(&dev, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET.0 as u32, 0);

    for frame in 1..=3 {
        let ins = sample_of(&in_tex);
        if let Err(e) = mft.ProcessInput(0, &ins, 0) {
            return format!("第{frame}帧 ProcessInput 失败: {e}");
        }
        let mut buf = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: if provides || !c.caller_sample {
                std::mem::ManuallyDrop::new(None)
            } else {
                std::mem::ManuallyDrop::new(Some(sample_of(&out_tex)))
            },
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        };
        let mut status = 0u32;
        match mft.ProcessOutput(0, std::slice::from_mut(&mut buf), &mut status) {
            Ok(()) => {
                let got = std::mem::ManuallyDrop::take(&mut buf.pSample);
                let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                let n = got.map(|s| s.GetBufferCount().unwrap_or(0)).unwrap_or(0);
                if frame == 1 {
                    // 第一帧成功就够说明配置可用；记下缓冲数与时戳能力。
                    return format!(
                        "OK(第{frame}帧成功, buffers={n}, 流信息 flags=0x{:X}, provides={provides})",
                        info.dwFlags
                    );
                }
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                let _ = std::mem::ManuallyDrop::take(&mut buf.pSample);
                let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                if frame == 3 {
                    return "NEED_MORE_INPUT ×3".into();
                }
            }
            Err(e) => {
                let _ = std::mem::ManuallyDrop::take(&mut buf.pSample);
                let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                return format!(
                    "第{frame}帧 ProcessOutput 失败: {e} (provides={provides}, caller={}, clean={})",
                    c.caller_sample, c.clean_input
                );
            }
        }
        // 释放引用，避免帧间累积。
        let _ = &ctx;
    }
    "未产出".into()
}

#[test]
#[ignore]
fn mft_d3d_manager_matrix() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).unwrap();

        let cases = vec![
            Case {
                name: "基线：不设 mgr，外部样本，采集纹理".into(),
                mgr_before_types: false,
                use_mgr: false,
                caller_sample: true,
                clean_input: false,
            },
            Case {
                name: "设 mgr(类型后)，外部样本，采集纹理".into(),
                mgr_before_types: false,
                use_mgr: true,
                caller_sample: true,
                clean_input: false,
            },
            Case {
                name: "设 mgr(类型后)，外部样本，干净纹理".into(),
                mgr_before_types: false,
                use_mgr: true,
                caller_sample: true,
                clean_input: true,
            },
            Case {
                name: "设 mgr(类型后)，MFT 提供样本".into(),
                mgr_before_types: false,
                use_mgr: true,
                caller_sample: false,
                clean_input: false,
            },
            Case {
                name: "设 mgr(类型前)，外部样本，采集纹理".into(),
                mgr_before_types: true,
                use_mgr: true,
                caller_sample: true,
                clean_input: false,
            },
            Case {
                name: "设 mgr(类型前)，MFT 提供样本".into(),
                mgr_before_types: true,
                use_mgr: true,
                caller_sample: false,
                clean_input: false,
            },
        ];

        for c in &cases {
            println!("{:52} -> {}", c.name, run_case(c));
        }

        MFShutdown().unwrap();
    }
}
