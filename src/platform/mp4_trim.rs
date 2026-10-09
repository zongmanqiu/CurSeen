//! MP4 **时间裁剪**：复用 Media Foundation 的 `IMFSourceReader` 读**已压缩**的样本，不解码，
//! 只把**保留区间**内的样本经 `MPEG4MediaSink` 重写成一份新 MP4。
//!
//! # 为什么是流拷贝（stream-copy）而不是重新编码
//!
//! - 录制器本来就把 MP4 交给系统 MF（`IMFSinkWriter` + H.264/AAC），这套组件是 OS 自带、
//!   **不进我们自己的二进制**——所以裁剪链路零新增体积。
//! - 流拷贝全程不碰编码器：内存占用是"一次处理一个样本"，峰值极小。
//! - 代价：视频裁剪点必须落在**关键帧（IDR）**边界，所以区间起点会向**更早**吸附到最近的
//!   关键帧（亚秒级，<1 个 GOP）；音频跟随吸附后的边界，音画保持同步。
//!
//! # 多保留区间（抠中间段）
//!
//! 保留区间可以是多段（`trim_ranges`）：被删掉中段后，后段样本的**时间戳要整体前移**，
//! 否则产物里那段是"定格/空转"。每段的位移 = 前面所有保留段时长之和（`remap_ts` 纯函数）。
//!
//! # 预览（`frame_rgba_at`）
//!
//! 裁剪窗要显示"这一秒画面长什么样"——这一步**必须解码**（压缩帧不能直接画）。走 MF 自带
//! 的 H.264 解码器（同样是 OS 组件），按时间 seek 后读出覆盖该时刻的那一帧，转成 RGBA
//! 给 windui 画。逐次调用各自开 reader：一次几十毫秒，比"常驻解码器"省内存，交互够用。
//!
//! ★★ **流索引不能写死 0**：SourceReader 的流下标由**文件里轨道的排列**决定，不是"先加
//! 的就在 0"。自家录屏实测音频在 0、视频在 1（写盘时视频先 `AddStream`，但 MP4 源解析
//! 器按自己的顺序报），写死 0 就是去音频流上问 `MF_MT_FRAME_SIZE` → 必然 `None`
//! （2026-09-30 真机："读不到画面尺寸"，预览全黑）。一律用
//! [`open_passthrough_reader`] 返回的 `video_idx`。
//!
//! # 写盘：为什么必须关掉 SinkWriter 的节流
//!
//! ★★ `IMFSinkWriter` **默认按播放速率节流**（`WriteSample` 会睡到"这一帧该呈现的时刻"）。
//! 录屏是为了实时播放，节流是对的；**裁剪是离线重写**，按实时速率写就是自找慢：实测一段
//! 13s / 13MB 的录屏，读样本 **0ms**、写样本 **155s**（2026-09-30 真机，用户报"保存卡死"，
//! 产物只有 79 字节的空壳——进程被杀在半路）。建 writer 时必须显式设
//! `MF_SINK_WRITER_DISABLE_THROTTLING`（[`trim_ranges_progress`]）。
//!
//! # MF 平台初始化
//!
//! ★ `MFStartup` 是进程级**引用计数**，但**必须有人起过**才能用任何 MF API。录制器只在
//!   录制线程起——而裁剪/预览跑在 UI 线程，本次会话**没录过**时 MF 从未启动，所有调用
//!   静默失败（2026-09-30 用户报"选了文件没反应"）。所以本模块每个入口都自己守一份
//!   [`MfGuard`]（RAII，多重入无害）。

use std::path::Path;
use std::time::Duration;

use windows::core::{Result as WinResult, GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;

/// MF 平台的 **RAII 引用计数守卫**（见模块头"MF 平台初始化"）。
///
/// ★ `pub(crate)`：`mp4_concat`（分段拼接）也要用，MF 的引用计数**全进程只该起一次**。
pub(crate) struct MfGuard;

impl MfGuard {
    pub(crate) fn start() -> Self {
        unsafe {
            let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        }
        MfGuard
    }
}

impl Drop for MfGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

pub(crate) const NS_PER_S: i64 = 10_000_000;

// ─────────────────── 纯函数（门禁钉在这里，不依赖 MF） ───────────────────

/// 起点吸附：把保留区间的**起点**向更早吸附到 ≤ 它的最近关键帧。
///
/// `starts` 必须升序。找不到更早的关键帧（区间头就在第一帧之前）就原样返回。
/// 关键帧时间戳直接取自源流，与样本时间戳同源，吸附不会引入半帧误差。
pub(crate) fn snap_start(start_ns: i64, keyframes: &[i64]) -> i64 {
    let mut best = start_ns;
    for &k in keyframes {
        if k <= start_ns {
            best = k;
        } else {
            break;
        }
    }
    best
}

/// 归并排序后重叠/相邻的区间（吸附后区间可能互相越界）。
pub(crate) fn merge_ranges(mut rs: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    rs.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(rs.len());
    for (s, e) in rs {
        if e <= s {
            continue;
        }
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// 源时间戳 → 产物时间戳。`ranges` 是吸附归并后的保留区间（升序不重叠）。
///
/// 每段的位移 = 该段起点 − 前面所有保留段时长之和，使各段在产物里**首尾相接**。
/// 落在保留区间之外的样本返回 `None`（调用方丢弃）。
pub(crate) fn remap_ts(ts_ns: i64, ranges: &[(i64, i64)]) -> Option<i64> {
    let mut base = 0i64;
    for &(s, e) in ranges {
        if ts_ns < s {
            return None;
        }
        if ts_ns < e {
            return Some(ts_ns - s + base);
        }
        base += e - s;
    }
    None
}

/// 总保留时长（100ns）。
pub(crate) fn kept_total_ns(ranges: &[(i64, i64)]) -> i64 {
    ranges.iter().map(|(s, e)| e - s).sum()
}

// ─────────────────── 时长 / 关键帧 ───────────────────

/// 建一个 SourceReader（每条流设回原生压缩类型，不让 MF 插解码器）。
///
/// `attrs` 非空时传给 reader（预览解码用它开 `ENABLE_ADVANCED_VIDEO_PROCESSING`，
/// 见 [`frame_rgba_at]）。
/// 返回 `(reader, video_idx, audio_idx)`。
///
/// ★ `pub(crate)`：拼接（`mp4_concat`）逐段建 reader 时也走这条"不解码"的路。
pub(crate) unsafe fn open_passthrough_reader(
    src: &Path,
    attrs: Option<&IMFAttributes>,
) -> WinResult<(IMFSourceReader, u32, Option<u32>)> {
    let url = HSTRING::from(src.as_os_str());
    let reader: IMFSourceReader = MFCreateSourceReaderFromURL(&url, attrs)?;
    let mut video_idx = None;
    let mut audio_idx = None;
    for idx in 0..4u32 {
        let mt = match reader.GetNativeMediaType(idx, 0) {
            Ok(mt) => mt,
            Err(_) => break,
        };
        let Ok(major) = mt.GetGUID(&MF_MT_MAJOR_TYPE) else {
            continue;
        };
        if major == MFMediaType_Video {
            video_idx = Some(idx);
        } else if major == MFMediaType_Audio {
            audio_idx = Some(idx);
        } else {
            continue;
        }
        let _ = reader.SetStreamSelection(idx, true);
        let _ = reader.SetCurrentMediaType(idx, None, &mt);
    }
    let v = video_idx
        .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    Ok((reader, v, audio_idx))
}

/// 读一帧（不解码），返回 `(时间戳, 是否关键帧, 是否到尾, 样本本体)`。
///
/// ★ `pub(crate)`：拼接复用（同 `open_passthrough_reader`）。
pub(crate) unsafe fn read_raw(
    reader: &IMFSourceReader,
    idx: u32,
) -> WinResult<(i64, bool, bool, Option<IMFSample>)> {
    let mut actual = 0u32;
    let mut flags = 0u32;
    let mut ts = 0i64;
    let mut sample = None;
    reader.ReadSample(
        idx,
        0,
        Some(&mut actual),
        Some(&mut flags),
        Some(&mut ts),
        Some(&mut sample),
    )?;
    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
        return Ok((ts, false, true, None));
    }
    let clean = sample
        .as_ref()
        .and_then(|s| s.GetUINT32(&MFSampleExtension_CleanPoint).ok())
        .map(|v| v != 0)
        .unwrap_or(false);
    Ok((ts, clean, false, sample))
}

/// 读一份 MP4 的时长（秒）。用于裁剪窗给时间轴定范围。
pub fn mp4_duration(src: &Path) -> Option<f64> {
    mp4_duration_ns(src).map(|ns| ns as f64 / NS_PER_S as f64)
}

/// [`mp4_duration`] 的 100ns 版本（裁剪窗要毫秒整数，走这里免去来回换算丢精度）。
pub fn mp4_duration_ns(src: &Path) -> Option<i64> {
    let _mf = MfGuard::start();
    unsafe {
        let (reader, video_idx, _audio) = match open_passthrough_reader(src, None) {
            Ok(x) => x,
            Err(e) => {
                log::warn!("mp4_duration：SourceReader 打不开 {}（{e}）", src.display());
                return None;
            }
        };
        let mut max_end_ns = 0i64;
        // 只扫视频流（屏幕录制必有视频，音频终点不会更晚）。
        for _ in 0..(1 << 22) {
            let (ts, _, eos, _) = read_raw(&reader, video_idx).unwrap_or((0, false, true, None));
            if eos {
                break;
            }
            max_end_ns = max_end_ns.max(ts);
        }
        if max_end_ns <= 0 {
            None
        } else {
            Some(max_end_ns)
        }
    }
}

// ─────────────────── 预览（解码一帧） ───────────────────

/// 解码 `t_s` 秒处的**那一帧**，返回 `(w, h, RGBA)`。
///
/// "那一帧"= 时间上覆盖 `t_s` 的帧（`ts ≤ t < ts+dur`）。seek 只能到关键帧，之后顺序读到
/// 目标为止（H.264 的 B/P 帧要靠前面的帧才能解出来，这是解码器的固有行为不是 bug）。
pub fn frame_rgba_at(src: &Path, t_s: f64) -> Result<(u32, u32, Vec<u8>), String> {
    let _mf = MfGuard::start();
    unsafe {
        // ★ 必须在**建 reader** 时开 `MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING`：
        //   默认配置下 Source Reader 只肯吐压缩原生格式或解码器直接支持的输出（H.264 解码器
        //   只出 NV12），要 RGB32 必须允许它插色彩转换器——否则 `SetCurrentMediaType(RGB32)`
        //   回 `MF_E_INVALIDMEDIATYPE`（0xC00D36B4，2026-09-30 真机实锤）。属性是建 reader
        //   那一刻读的，事后补设无效。
        let mut attrs_opt: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs_opt, 10).map_err(|e| format!("建属性失败：{e}"))?;
        let attrs = attrs_opt.ok_or("建属性返回空对象".to_string())?;
        let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1);
        // ★ 用解析出来的**视频流下标**（见模块头"流索引不能写死 0"）。
        let (reader, video_idx, audio_idx) =
            open_passthrough_reader(src, Some(&attrs)).map_err(|e| format!("打不开视频：{e}"))?;
        log::debug!("frame_rgba_at：视频流 {video_idx}，音频流 {audio_idx:?}");
        // 原生（压缩）类型：要拿画面尺寸，还要把它的属性整包拷进输出类型。
        let native = reader
            .GetNativeMediaType(video_idx, 0)
            .map_err(|e| format!("读原生媒体类型失败：{e}"))?;
        let (w, h) = frame_size(&native).ok_or("读不到画面尺寸")?;
        // 让这条流吐解码后的 RGB32。
        //
        // ★ 必须整包拷贝原生属性再换 SUBTYPE：只给 MAJOR/SUBTYPE 两个字段是"残缺"的
        //   媒体类型，`SetCurrentMediaType` 直接回 `MF_E_INVALIDMEDIATYPE`（0xC00D36B4）。
        // ★ 还必须显式给 `MF_MT_DEFAULT_STRIDE`：RGB 步长有歧义（可以是正/负步长两种
        //   布局），MF 不肯替你猜——2026-09-30 真机实锤，不指定则自家录像一帧都解不出来。
        let mt = MFCreateMediaType().map_err(|e| format!("建媒体类型失败：{e}"))?;
        native
            .CopyAllItems(&mt)
            .map_err(|e| format!("拷属性失败：{e}"))?;
        let _ = mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
        let _ = mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32);
        let _ = mt.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4);
        reader
            .SetCurrentMediaType(video_idx, None, &mt)
            .map_err(|e| format!("设解码格式失败：{e}"))?;
        // stride 为正 = 顶行在前（top-down，MF 的 RGB 约定）；为负 = 底行在前（BI_RGB DIB）。
        // 上面已显式设了正步长，这里实际读回时恒为 w*4；仍读实际值防解码器改主意。
        let stride = reader
            .GetCurrentMediaType(video_idx)
            .ok()
            .and_then(|cur| cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok())
            .unwrap_or(w * 4) as i32;

        // seek 到目标。★ SDK（mfreadwrite.idl）注释明示：时间格式传 `GUID_NULL` 就是
        // 100ns 单位，不需要另找 `GUID_TIME`（SDK 头里也没有这个名字）。
        let target = (t_s.max(0.0) * NS_PER_S as f64) as i64;
        let mut pv = PROPVARIANT::default();
        // ★ union 字段包在 `ManuallyDrop` 里，写入必须显式 `*`（编译器不自动 DerefMut）。
        (*pv.Anonymous.Anonymous).vt = VT_I8;
        (*pv.Anonymous.Anonymous).Anonymous.hVal = target;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &pv)
            .map_err(|e| format!("定位失败：{e}"))?;

        // 顺序读到覆盖目标的帧。B/P 帧依赖前方参考帧，解码器必须从关键帧顺序吐。
        for _ in 0..512 {
            let mut actual = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader
                .ReadSample(
                    video_idx,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| format!("读帧失败：{e}"))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Err("已到片尾".to_string());
            }
            let Some(sample) = sample else { continue };
            let dur = sample.GetSampleDuration().unwrap_or(0);
            let covered = ts <= target && (ts + dur > target || dur == 0 && ts >= target - 1);
            let reached = ts >= target;
            if !covered && !reached {
                continue;
            }
            let rgba = sample_to_rgba(&sample, w, h, stride)?;
            return Ok((w, h, rgba));
        }
        Err("seek 后迟迟读不到目标帧".to_string())
    }
}

/// 从媒体类型里抠 `MF_MT_FRAME_SIZE`（高 32 位宽、低 32 位高）。
unsafe fn frame_size(mt: &IMFMediaType) -> Option<(u32, u32)> {
    let v = mt.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
    Some(((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32))
}

/// IMFSample → RGBA。MF 的 RGB32 内存布局是 **B,G,R,A**。
///
/// 行序：**MF 的 stride 约定与 GDI 相反**——正 stride = 顶行在前（top-down），
/// 负 stride = 底行在前（BI_RGB DIB）。我们申请时给的是正步长，正常就是顺序读。
unsafe fn sample_to_rgba(
    sample: &IMFSample,
    w: u32,
    h: u32,
    stride: i32,
) -> Result<Vec<u8>, String> {
    // `ConvertToContiguousBuffer` 返回的就是 IMFMediaBuffer，不需要再 cast。
    let buffer: IMFMediaBuffer = sample
        .ConvertToContiguousBuffer()
        .map_err(|e| format!("样本合并失败：{e}"))?;
    let mut ptr = std::ptr::null_mut::<u8>();
    let mut max_len = 0u32;
    let mut cur_len = 0u32;
    buffer
        .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
        .map_err(|e| format!("锁缓冲失败：{e}"))?;
    let need = (stride.unsigned_abs() as usize) * h as usize;
    if (cur_len as usize) < need {
        let _ = buffer.Unlock();
        return Err(format!("缓冲比预期小：{cur_len} < {need}"));
    }
    let src = std::slice::from_raw_parts(ptr, need);
    let abs_stride = stride.unsigned_abs() as usize;
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h as usize {
        // 正 stride = 顶行在前 → 顺序读；负 stride = 底行在前 → 行序倒过来。
        let src_row = if stride < 0 {
            h as usize - 1 - row
        } else {
            row
        };
        let line = &src[src_row * abs_stride..];
        let dst = &mut out[row * w as usize * 4..][..w as usize * 4];
        for (px, d) in dst.chunks_exact_mut(4).enumerate() {
            let s = &line[px * 4..px * 4 + 4];
            // BGRA → RGBA；alpha 来自源（屏幕录制恒不透明，缺失时补 255）。
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = if s[3] == 0 { 255 } else { s[3] };
        }
    }
    let _ = buffer.Unlock();
    Ok(out)
}

// ─────────────────── 裁剪（多保留区间，流拷贝） ───────────────────

/// 裁剪结果（给日志/提示用）。
pub struct TrimStats {
    /// 产物保留时长（秒）。
    pub kept_s: f64,
}

/// 把 `src` 的**保留区间**（秒，可多段、乱序、重叠）流拷贝成 `dst`。
///
/// - 区间起点向更早吸附到关键帧（见模块头），音频跟随吸附后的边界 → 音画同步。
/// - 各段时间戳重映射成首尾相接（`remap_ts`），删中间段不会留下"定格"。
/// - **不重新编码**：画质零损失，内存"一次一个样本"。
pub fn trim_ranges(src: &Path, dst: &Path, keep_s: &[(f64, f64)]) -> WinResult<TrimStats> {
    trim_ranges_progress(src, dst, keep_s, &mut |_| {})
}

/// [`trim_ranges`] 的带进度版：`on` 收到 `0.0..=1.0` 的完成度。
///
/// 裁剪是**秒级**操作（见模块头"写盘为什么必须关节流"），UI 必须能显示进度，所以进度
/// 走回调而不是返回值。回调在**调用方线程**里同步触发（可能来自工作线程）。
pub fn trim_ranges_progress(
    src: &Path,
    dst: &Path,
    keep_s: &[(f64, f64)],
    on: &mut dyn FnMut(f64),
) -> WinResult<TrimStats> {
    let _mf = MfGuard::start();
    unsafe {
        let (reader, video_idx, audio_idx) = open_passthrough_reader(src, None)?;

        // ① 预扫一遍视频流拿关键帧时间戳（不解码，很快）。顺带记末帧时间戳——进度要以
        //   "走到全片几分之几"为刻度，UI 上才是匀速的。
        let mut keyframes = Vec::new();
        let mut total_ns = 0i64;
        loop {
            let (ts, clean, eos, _) = read_raw(&reader, video_idx)?;
            if eos {
                break;
            }
            total_ns = total_ns.max(ts);
            if clean {
                keyframes.push(ts);
            }
        }

        // ② 吸附 + 归并。
        let mut ranges: Vec<(i64, i64)> = keep_s
            .iter()
            .map(|&(s, e)| {
                let s_ns = (s.max(0.0) * NS_PER_S as f64) as i64;
                let e_ns = (e.max(0.0) * NS_PER_S as f64) as i64;
                (snap_start(s_ns, &keyframes), e_ns)
            })
            .collect();
        ranges = merge_ranges(ranges);
        if ranges.is_empty() {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }

        // ② 预扫已把视频流读到 EOF，**必须回卷到 0** 再进拷贝循环——否则第一条流一个
        //   样本都读不到（2026-09-30 真机实锤，表现为 Finalize 报"未处理任何样本"）。
        let mut pv0 = PROPVARIANT::default();
        (*pv0.Anonymous.Anonymous).vt = VT_I8;
        (*pv0.Anonymous.Anonymous).Anonymous.hVal = 0;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &pv0)
            .map_err(|e| {
                log::warn!("trim：回卷失败：{e}");
                e
            })?;

        // ③ 目标：MP4 封装器直吃压缩样本。
        let video_type = reader.GetCurrentMediaType(video_idx)?;
        let audio_type = match audio_idx {
            Some(i) => Some(reader.GetCurrentMediaType(i)?),
            None => None,
        };
        let dst_url = HSTRING::from(dst.as_os_str());
        log::debug!(
            "trim：预扫完成，关键帧 {} 个，保留段 {:?}",
            keyframes.len(),
            ranges
        );
        let byte_stream: IMFByteStream = MFCreateFile(
            MF_ACCESSMODE_WRITE,
            MF_OPENMODE_DELETE_IF_EXIST,
            MF_FILEFLAGS_NONE,
            &dst_url,
        )
        .map_err(|e| {
            log::warn!("trim：MFCreateFile 失败：{e}");
            e
        })?;
        // ★ 第 2/3 参传 **None**：这两个参数是"预建轨道"——传了类型，sink 就先建好 0/1 两条
        //   空轨，下面 `AddStream` 又会追加 2/3 两条真轨，产物变成 4 条轨（前两条零样本）。
        //   播放器默认挑第一条视频/音频轨 ⇒ **黑屏 + 无声**；MF SourceReader 是扫描式的，
        //   能找到真轨，所以自家探针看不出来（2026-09-30 用户实测裁剪产物黑屏无声）。
        let sink: IMFMediaSink = MFCreateMPEG4MediaSink(&byte_stream, None, None).map_err(|e| {
            log::warn!("trim：MFCreateMPEG4MediaSink 失败：{e}");
            e
        })?;
        // ★★ SinkWriter **默认按播放速率节流写盘**（`MF_SINK_WRITER_DISABLE_THROTTLING`
        //    未设）：`WriteSample` 会睡到"该帧该呈现的时刻"才返回——实测一段 13s / 13MB
        //    的录屏，**读样本 0ms、写样本 155s**（2026-09-30 真机，用户报"保存卡死"）。
        //    裁剪是离线重写，没有"按实时播"这回事，必须显式关掉节流。
        let mut w_attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut w_attrs, 2)?;
        let writer: IMFSinkWriter = match &w_attrs {
            Some(a) => {
                let _ = a.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1);
                MFCreateSinkWriterFromMediaSink(&sink, Some(a))
            }
            None => MFCreateSinkWriterFromMediaSink(&sink, None),
        }
        .map_err(|e| {
            log::warn!("trim：MFCreateSinkWriterFromMediaSink 失败：{e}");
            e
        })?;
        let v_out = writer.AddStream(&video_type).map_err(|e| {
            log::warn!("trim：AddStream(视频) 失败：{e}");
            e
        })?;
        let a_out = match &audio_type {
            Some(at) => Some(writer.AddStream(at).map_err(|e| {
                log::warn!("trim：AddStream(音频) 失败：{e}");
                e
            })?),
            None => None,
        };
        writer.BeginWriting().map_err(|e| {
            log::warn!("trim：BeginWriting 失败：{e}");
            e
        })?;

        // ④ 逐流拷贝 + 时间戳重映射。进度按"流"分账：视频占大头（样本体积在那儿），
        //   有音频时给它留 [`AUDIO_SPAN`]，没有音频则视频跑满 0→1。
        let v_span = if a_out.is_some() {
            1.0 - AUDIO_SPAN
        } else {
            1.0
        };
        copy_stream(
            &reader, video_idx, true, &ranges, &writer, v_out, total_ns, 0.0, v_span, on,
        )?;
        if let (Some(ai), Some(ao)) = (audio_idx, a_out) {
            copy_stream(
                &reader, ai, false, &ranges, &writer, ao, total_ns, v_span, AUDIO_SPAN, on,
            )?;
        }
        on(1.0);
        writer.Finalize()?;
        Ok(TrimStats {
            kept_s: kept_total_ns(&ranges) as f64 / NS_PER_S as f64,
        })
    }
}

/// 进度条里留给**音频流**的那一段（视频占剩下的）。
///
/// 音频样本又小又密，按"样本个数"分账会让进度条在音频段飞过去；按时间分账又不准
/// （两流时长相同）。这里直接切一块固定比例——用户看到的是"还在动"，不是精确物理进度。
const AUDIO_SPAN: f64 = 0.15;

/// 把一条流的保留样本写进 `writer`（时间戳已重映射）。
///
/// `total_ns / base / span`：本流在总进度条里占 `[base, base+span]`，刻度按**样本时间戳
/// 走到全片几分之几**算（跳过删除段时进度会跳一下，这是对的——那段确实不用处理）。
#[allow(clippy::too_many_arguments)]
unsafe fn copy_stream(
    reader: &IMFSourceReader,
    idx: u32,
    is_video: bool,
    ranges: &[(i64, i64)],
    writer: &IMFSinkWriter,
    out_idx: u32,
    total_ns: i64,
    base: f64,
    span: f64,
    on: &mut dyn FnMut(f64),
) -> WinResult<()> {
    let mut prev_in_range = false;
    let (mut n_read, mut n_skip_ts, mut n_skip_kf, mut n_write) = (0u32, 0u32, 0u32, 0u32);
    let (mut ms_read, mut ms_write) = (0u128, 0u128);
    let t0 = std::time::Instant::now();
    loop {
        let t = std::time::Instant::now();
        let (ts, clean, eos, sample) = read_raw(reader, idx)?;
        ms_read += t.elapsed().as_millis();
        if eos {
            log::debug!(
                "copy_stream[{idx}]：读 {n_read}，删区间外 {n_skip_ts}，等关键帧 {n_skip_kf}，写出 {n_write}（读 {ms_read}ms / 写 {ms_write}ms / 总 {}ms）",
                t0.elapsed().as_millis()
            );
            return Ok(());
        }
        n_read += 1;
        // 进度按时间戳走（不是样本计数）：跳过删除段时它会前跳，那正说明"那段不用处理"。
        let frac = if total_ns > 0 {
            (ts.max(0) as f64 / total_ns as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        on(base + span * frac);
        if n_read % 64 == 0 {
            log::debug!(
                "copy_stream[{idx}] 进度 n={n_read} 写={n_write}（读 {ms_read}ms / 写 {ms_write}ms）"
            );
        }
        match remap_ts(ts, ranges) {
            None => {
                n_skip_ts += 1;
                prev_in_range = false;
            }
            Some(new_ts) => {
                // 视频在刚进入一段保留区间时必须从关键帧起写（吸附已保证起点是关键帧，
                // 这里兜底防"时间戳微差"导致首帧不可解）。
                if is_video && !prev_in_range && !clean {
                    n_skip_kf += 1;
                    continue;
                }
                let Some(sample) = sample else { continue };
                let _ = sample.SetSampleTime(new_ts);
                let t = std::time::Instant::now();
                writer.WriteSample(out_idx, &sample).map_err(|e| {
                    log::warn!("copy_stream[{idx}]：WriteSample @ {new_ts} 失败：{e}");
                    e
                })?;
                ms_write += t.elapsed().as_millis();
                n_write += 1;
                prev_in_range = true;
            }
        }
    }
}

/// 兼容旧调用点（单区间）。
pub fn trim_mp4(src: &Path, dst: &Path, start: Duration, end: Duration) -> WinResult<TrimStats> {
    trim_ranges(src, dst, &[(start.as_secs_f64(), end.as_secs_f64())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 时间戳一律写成 `秒数 * S`：直接抄 100ns 整数极容易看错量级（曾经写成 `2_000_000_0`，
    /// 那既不是 2s 也不是 2s 的 100ns 计数）。
    const S: i64 = NS_PER_S;

    #[test]
    fn snap_start_picks_the_latest_keyframe_at_or_before() {
        let kf = [0, 2 * S, 4 * S]; // 0s / 2s / 4s
        assert_eq!(snap_start(2 * S + S / 2, &kf), 2 * S, "吸附到前一个关键帧");
        assert_eq!(snap_start(2 * S, &kf), 2 * S, "正好落在关键帧上不移动");
        assert_eq!(snap_start(S, &kf), 0, "关键帧之前 → 吸到 0");
        assert_eq!(snap_start(9 * S, &[0]), 0, "没有更早关键帧就原样");
    }

    /// `MFCreateMPEG4MediaSink` 的第 2/3 参是**预建轨道**：传了媒体类型，sink 先建 0/1 两条
    /// 空轨，`AddStream` 再追加真轨 ⇒ 产物 4 条轨、前两条零样本。播放器挑第一条轨 ⇒
    /// **黑屏 + 无声**（MF SourceReader 扫描式所以看不出来，2026-09-30 用户实测中招）。
    /// 无头环境造不了 MF 会话，把这条纪律钉在源码结构上。
    #[test]
    fn sink_is_created_without_pre_seeded_tracks() {
        let src = include_str!("mp4_trim.rs");
        let create = src
            .lines()
            .find(|l| l.contains("MFCreateMPEG4MediaSink(&byte_stream"))
            .expect("必须能找到 MFCreateMPEG4MediaSink 调用");
        assert!(
            create.contains("None, None"),
            "MP4 sink 不得预建轨道（第 2/3 参必须传 None）：{create}"
        );
    }

    #[test]
    fn remap_ts_makes_ranges_contiguous_and_drops_the_rest() {
        // 保留 [0,2s) 和 [4,5s)：产物里 4s 处的样本应落在 2s。
        let rs = vec![(0, 2 * S), (4 * S, 5 * S)];
        assert_eq!(remap_ts(0, &rs), Some(0));
        assert_eq!(remap_ts(19 * S / 10, &rs), Some(19 * S / 10), "第一段原样");
        assert_eq!(remap_ts(4 * S, &rs), Some(2 * S), "第二段前移 2s");
        assert_eq!(remap_ts(49 * S / 10, &rs), Some(29 * S / 10));
        assert_eq!(remap_ts(3 * S, &rs), None, "被删的中间段");
        assert_eq!(remap_ts(5 * S, &rs), None, "端点不包含");
    }

    #[test]
    fn merge_ranges_sorts_unions_and_drops_empties() {
        assert_eq!(
            merge_ranges(vec![(40, 50), (0, 10), (8, 20), (5, 5)]),
            vec![(0, 20), (40, 50)]
        );
        assert!(merge_ranges(vec![]).is_empty());
        assert_eq!(merge_ranges(vec![(10, 10)]), Vec::<(i64, i64)>::new());
    }

    #[test]
    fn kept_total_sums_range_lengths() {
        assert_eq!(kept_total_ns(&[(0, 10), (20, 35)]), 25);
    }
}
