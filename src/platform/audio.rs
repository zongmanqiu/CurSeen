//! 点击音效播放（WAV / MP3 / 其它系统可解码的音频格式）。
//!
//! 对齐 Python `trail/controller.py` 的 `_play_sound`：按名选文件、逐键音量、重复点击时
//! "打断重播"。差异在于播放后端——Python 用 Qt `QSoundEffect`，我们直接调 Win32
//! （Cargo.toml 里明确不引 rodio/cpal 音频栈）。
//!
//! # 六个必须交代的设计点
//!
//! 1. **素材在目录里，运行期读**。声音文件放在 `<exe 同级>/assets/Sounds/`，与拖尾
//!    Lua 脚本、悬浮标图片同一套版图约定（见 `core::paths`）。用户丢一个文件进去就能在
//!    面板里选到，**扩展不需要重编译**——这是用户明确要的能力。
//!    ★ 这里推翻了 2.0.0 早期的"编译期 `include_bytes!` 内嵌"方案：内嵌确实更快、也不怕
//!    exe 被单独拷走，但它让"用户自定义声音"根本不可能实现。代价用两点补偿：**按需载入**
//!    （没被选过的文件永不读盘，启动零 IO）＋ **一次泄漏常驻**（同一文件只读一次）。
//! 2. **只收 Windows 原生就能播的格式，绝不为此引第三方解码库**。用户口径：
//!    "没必要强行读取，只要读取 Windows 能直接播放的就好，也不想为了小众格式再装额外的库"。
//!    于是 [`SUPPORTED_EXT`] **就是**"本机开箱能播"的那几个（wav/wave/mp3/wma/m4a/aac/
//!    aif/aiff/mid/midi）；`.wv`/`.ape`/`.flac` 这类系统没解码器的**根本不进列表**——
//!    不是"列出来但放不出声"，而是压根看不见（宁可没有这一项，也不给用户一个死选项）。
//!    播放后端分两档：
//!    - **WAV** 我们自己解析（RIFF/PCM），走 `PlaySoundW` + `SND_MEMORY`，音量靠采样定标。
//!    - **其它格式**交给 MCI（`mciSendStringW`）：有系统解码器就能播，音量用
//!      `setaudio ... volume`（逐声音，不碰设备总音量）。两个后端都是 Windows 自带，
//!      **零新增依赖**。
//! 3. **音量**。`PlaySoundW` 没有音量参数，而 `waveOutSetVolume` 是设备级（会连带改别的
//!    程序）——所以 WAV 走"按 10% 档生成一份缩放后的样本再播"。步进取 10%（11 档）而不是
//!    1%：人对 5% 以内的响度差本就分辨不出，而按 1% 缓存会让内存上限涨 10 倍。
//! 4. **缓存只增不删**（`Box::leak`）。`SND_ASYNC | SND_MEMORY` 要求那块内存在声音播完之前
//!    一直有效；回收条目会让正在播的声音读到已释放内存。上界 = 目录里被用过的文件体积
//!    之和 ＋ 这些名字 × 11 档，且只有真正点过的组合才会分配。
//! 5. **播放失败是静默的，只有"没有解码器"打一条日志**。名字拼错、文件被删、音量 0
//!    ——一律返回 `false` 不发声，绝不 panic（与 Python `_play_sound` 一致）。唯独
//!    "系统解不了这个格式"会 `warn` 一次（同一名字只打一次，不然每次点击都刷屏），
//!    因为那是用户**看不见也猜不到**的失败。
//! 6. **MCI 用一个固定别名**（[`MCI_ALIAS`]）。① 与 `PlaySoundW` 语义一致：新声音打断
//!    旧声音；② 不必为每个文件名生成合法别名（MCI 别名不能含空格/引号）。代价是每次
//!    播放都要 `close` 再 `open`，所以整段操作用 [`MCI_LOCK`] 串起来。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::Media::Multimedia::mciSendStringW;

/// 收哪些扩展名（**小写**）。顺序即优先级：同名不同后缀时取靠前的那个。
///
/// ★ 这张表**就是**"Windows 开箱能播"的集合——列表里没有的格式（`.wv` WavPack、`.ape`、
/// `.flac`…）系统没解码器，我们也不打算为它们引第三方库，所以**连扫都不扫**：不进列表，
/// 就不会出现"选了却放不出声"的死选项。要支持新格式：先把扩展名加到这里，再确认本机
/// 真有它的解码器（`mciSendStringW` open 得开）。
pub(crate) const SUPPORTED_EXT: &[&str] = &[
    "wav", "wave", "mp3", "wma", "m4a", "aac", "aif", "aiff", "mid", "midi",
];

/// MCI 别名（全局共用，见文件头第 6 点）。
const MCI_ALIAS: &str = "curseen_snd";

/// 一个声音条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEntry {
    /// 文件名 stem（不含扩展名）。**配置里存的就是它**（见 `path_in`）。
    pub stem: String,
    /// 小写扩展名（不含点）。
    pub ext: String,
}

/// 扫描声音目录，返回按名排序的条目。
///
/// 目录不存在 → 空列表（静默，与 `core::paths` 的"路径拼错是静默失败"一致）。
pub fn list_audio(dir: &Path) -> Vec<AudioEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = ext_of(&path) else {
            continue;
        };
        if !SUPPORTED_EXT.contains(&ext.as_str()) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push(AudioEntry {
            stem: stem.to_string(),
            ext,
        });
    }
    // 同名不同后缀（`boom.wav` + `boom.mp3`）：按下标优先级只留靠前的那个，
    // 否则下拉里会出现两项、解析到同一个 id，选中哪项都分不清。
    out.sort_by(|a, b| {
        a.stem
            .cmp(&b.stem)
            .then_with(|| ext_rank(&a.ext).cmp(&ext_rank(&b.ext)))
    });
    out.dedup_by(|a, b| a.stem == b.stem);
    out
}

/// 扩展名在 [`SUPPORTED_EXT`] 里的位置（越小越优先）。
fn ext_rank(ext: &str) -> usize {
    SUPPORTED_EXT
        .iter()
        .position(|e| *e == ext)
        .unwrap_or(usize::MAX)
}

/// 小写扩展名（不含点）。无扩展名返回 `None`。
fn ext_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// 声音名（stem）→ 该目录下的实际文件路径。
///
/// 配置里只存 stem（不带扩展名），所以这里按 [`SUPPORTED_EXT`] 的顺序挨个试。
/// Windows 文件系统大小写不敏感，`boom.WAV` 也能被 `boom.wav` 探到，故无需枚举大小写。
fn path_in(dir: &Path, name: &str) -> Option<PathBuf> {
    SUPPORTED_EXT
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|p| p.is_file())
}

/// 声音名字 → 实际文件路径。
fn path_for(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    // 老配置 / 手改配置里可能写着绝对路径，能读到就照用。
    let raw = Path::new(name);
    if raw.is_absolute() {
        return raw.is_file().then(|| raw.to_path_buf());
    }
    path_in(&crate::core::paths::sounds_dir(), name)
}

/// 读（并缓存）声音文件的原始字节；文件不存在返回 `None`。
///
/// 名字不再是编译期常量，没法再用 `include_bytes!` 直接给出 `'static` 切片，于是首次
/// 用到时读盘、`Box::leak` 成 `'static`（见文件头第 1、4 点）。
fn raw(name: &str) -> Option<&'static [u8]> {
    type RawCache = Mutex<HashMap<String, Option<&'static [u8]>>>;
    static CACHE: OnceLock<RawCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(name) {
            return *hit;
        }
    }
    let loaded = load(name);
    if let Ok(mut map) = cache.lock() {
        map.insert(name.to_string(), loaded);
    }
    loaded
}

/// 真正的读盘 + 泄漏常驻。见文件头第 1、4 点。
fn load(name: &str) -> Option<&'static [u8]> {
    let bytes = std::fs::read(path_for(name)?).ok()?;
    Some(&*Box::leak(bytes.into_boxed_slice()))
}

/// 是否存在该音效（看文件在不在，不读内容）。
pub fn has(name: &str) -> bool {
    path_for(name).is_some()
}

/// 播放音效。`volume_pct` 取 0–100（超出即钳制）。
///
/// 返回是否真的发起了播放：名不存在、音量为 0、或格式解不了时返回 `false`——静默失败
/// 而非报错，与 Python `_play_sound` 遇到缺文件时 `return` 一致（见文件头第 5 点）。
pub fn play(name: &str, volume_pct: u32) -> bool {
    let level = volume_pct.min(100) as u8;
    if level == 0 {
        return false;
    }
    let Some(path) = path_for(name) else {
        return false;
    };
    match ext_of(&path).as_deref() {
        // WAV 我们自己解码 + 采样定标（音量为逐样本缩放，最精确）。
        Some("wav" | "wave") => play_wav(name, level),
        // 其余交给 MCI：能播的靠系统解码器，音量靠 setaudio。
        _ => play_via_mci(&path, name, level),
    }
}

/// 播放 WAV：按音量档取（或生成）缩放后的样本，交给 `PlaySoundW` 异步播。
fn play_wav(name: &str, level: u8) -> bool {
    let Some(src) = raw(name) else {
        return false;
    };
    let data = if level >= 100 {
        src
    } else {
        match scaled(name, level) {
            Some(d) => d,
            None => return false,
        }
    };
    // SAFETY: `data` 的生命周期是 `'static`（由 `Box::leak` 产生且永不回收），故异步播放
    // 期间内存始终有效。WAV 是字节流，winmm 按字节读，`*const u16` 只是 `PCWSTR` 的载体类型。
    unsafe {
        let _ = PlaySoundW(
            PCWSTR::from_raw(data.as_ptr().cast::<u16>()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
    true
}

/// MCI 打开的结果：能不能播、能不能单独调音量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opened {
    /// 打开了，且 `setaudio` 可用（走 `type mpegvideo`）。
    WithVolume,
    /// 打开了，但驱动不认 `setaudio`（退回自动选设备）——能出声，音量为系统默认。
    NoVolume,
    /// 打不开（文件损坏 / 内容与扩展名不符 / 系统缺该解码器）。
    Failed,
}

/// 串行化 MCI 的 `close/open/play` 序列（别名是全局共用的，见文件头第 6 点）。
fn mci_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 发一条 MCI 命令。`Ok(返回串)` / `Err(MCI 错误码)`。
fn mci_send(cmd: &str) -> Result<String, u32> {
    let mut buf = [0u16; 512];
    let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` 以 NUL 结尾且在本调用期间存活；`buf` 的长度即返回缓冲上限，
    // windows-rs 把它一并传给 winmm。回调传 `None`（MCI 通知不需要）。
    let code = unsafe { mciSendStringW(PCWSTR::from_raw(wide.as_ptr()), Some(&mut buf), None) };
    if code == 0 {
        let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
        Ok(String::from_utf16_lossy(&buf[..n]))
    } else {
        Err(code)
    }
}

/// 打开一个音频文件到全局别名上（先关掉上一次的）。
///
/// 先试 `type mpegvideo`（可 `setaudio` 调音量），失败再退回自动选设备（只求能出声），
/// 两个都失败才算打不开。实测：WAV/MP3 两条路都通；WavPack `.wv` 两条都失败——所以
/// `.wv` 压根不在 [`SUPPORTED_EXT`] 里，走不到这里。
fn mci_open(path: &Path) -> Opened {
    // 路径里的引号会破坏命令串（Windows 文件名本就不允许引号，这里只是防御）。
    let p = path.display().to_string().replace('"', "");
    if mci_send(&format!("open \"{p}\" type mpegvideo alias {MCI_ALIAS}")).is_ok() {
        return Opened::WithVolume;
    }
    if mci_send(&format!("open \"{p}\" alias {MCI_ALIAS}")).is_ok() {
        return Opened::NoVolume;
    }
    Opened::Failed
}

/// 非 WAV：走 MCI 播放（音量经 `setaudio`，0–100 映射到 MCI 的 0–1000）。
fn play_via_mci(path: &Path, name: &str, level: u8) -> bool {
    let _guard = mci_lock().lock();
    // 别名可能是上次播放留下的：先关掉再开（同一个别名上 `open` 不能重复执行）。
    let _ = mci_send(&format!("close {MCI_ALIAS}"));
    match mci_open(path) {
        Opened::Failed => {
            warn_no_decoder(name, path);
            false
        }
        kind => {
            if kind == Opened::WithVolume {
                let _ = mci_send(&format!(
                    "setaudio {MCI_ALIAS} volume to {}",
                    u32::from(level) * 10
                ));
            }
            mci_send(&format!("play {MCI_ALIAS} from 0")).is_ok()
        }
    }
}

/// 记一条"这个文件播不了"日志；同一名字只打一次。
///
/// 正常情况不该出现——[`SUPPORTED_EXT`] 里的格式系统都能解。真出现了（文件损坏、内容
/// 与扩展名不符、系统被精简掉了解码器），用户是**看不见也猜不到**的，所以留一条日志。
fn warn_no_decoder(name: &str, path: &Path) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let set = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut set) = set.lock() {
        if set.insert(name.to_string()) {
            log::warn!(
                "按键声音「{name}」打不开（{}）：文件可能损坏，或内容与扩展名不符。\
                 请换一个 wav / mp3 放进 assets/Sounds/",
                path.display()
            );
        }
    }
}

/// 缩放缓存表：(声音名, 音量档) → 已缩放的 WAV 字节。
type ScaleCache = Mutex<HashMap<(String, u8), &'static [u8]>>;

/// 按 10% 档位缓存缩放后的样本。
///
/// `Mutex` 而非线程局部：钩子线程只发事件、不播音，但设计上不依赖这一点（`play` 可能被
/// 任何线程调用，`PlaySoundW` 本身是线程安全的）。
fn scaled(name: &str, level: u8) -> Option<&'static [u8]> {
    static CACHE: OnceLock<ScaleCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = (name.to_string(), level);
    if let Ok(map) = cache.lock() {
        if let Some(d) = map.get(&key) {
            return Some(*d);
        }
    }
    let src = raw(name)?;
    let data = scale_wav(src, level)?;
    // 有意泄漏：见文件头第 4 点。上界 = 用过的名字 × 11 档。
    let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
    if let Ok(mut map) = cache.lock() {
        map.insert(key, leaked);
    }
    Some(leaked)
}

/// 把 16-bit PCM 的 `data` 块逐样本乘以 `level/100`。非 16-bit PCM 返回 `None`。
fn scale_wav(src: &[u8], level: u8) -> Option<Vec<u8>> {
    if bits_per_sample(src) != Some(16) {
        return None;
    }
    let (off, len) = find_chunk(src, b"data")?;
    let mut out = src.to_vec();
    let gain = level as f32 / 100.0;
    let end = off.saturating_add(len);
    let mut i = off;
    while i + 2 <= end {
        let s = i16::from_le_bytes([out[i], out[i + 1]]);
        let v = ((s as f32) * gain).round().clamp(-32768.0, 32767.0) as i16;
        out[i..i + 2].copy_from_slice(&v.to_le_bytes());
        i += 2;
    }
    Some(out)
}

/// 读 `fmt ` 块里的位深（偏移 +14 的 u16）。
fn bits_per_sample(buf: &[u8]) -> Option<u16> {
    let (off, len) = find_chunk(buf, b"fmt ")?;
    if len < 16 {
        return None;
    }
    Some(u16::from_le_bytes([buf[off + 14], buf[off + 15]]))
}

/// 在 RIFF 块链里定位指定 id 的块，返回 `(数据偏移, 数据长度)`。
///
/// 必须走块链而不是取固定偏移：真实文件在 `fmt ` 与 `data` 之间夹着 `LIST/INFO`
/// （Lavf 写入的编码器信息），硬编码偏移会读到 LIST 的内容。
fn find_chunk(buf: &[u8], want: &[u8; 4]) -> Option<(usize, usize)> {
    if buf.len() < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size =
            u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as usize;
        let body = pos + 8;
        if id == want {
            let len = size.min(buf.len().saturating_sub(body));
            return Some((body, len));
        }
        // 块按偶数字节对齐，奇数长度后有一字节填充。
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 16-bit PCM 的合法 WAV。
    fn synth_wav(samples: &[i16]) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&2u16.to_le_bytes()); // 2 声道
        v.extend_from_slice(&44100u32.to_le_bytes());
        v.extend_from_slice(&176400u32.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("curseen_audio_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 声音目录里的每个 `.wav` 都必须能载入、且是 16-bit PCM。
    ///
    /// 依赖 CWD：`cargo test` 从 crate 根（`main`）跑，`sounds_dir()` 回退到
    /// `assets/Sounds` 正好命中。素材没跟着分发时本测试会红——那正是要抓的情况。
    #[test]
    fn every_wav_in_the_sound_dir_is_a_usable_16bit_pcm() {
        let dir = crate::core::paths::sounds_dir();
        let wavs: Vec<AudioEntry> = list_audio(&dir)
            .into_iter()
            .filter(|e| e.ext == "wav")
            .collect();
        assert!(
            wavs.len() >= 14,
            "声音目录 {dir:?} 里只扫到 {} 个 .wav（素材没跟过来？）",
            wavs.len()
        );
        // 出厂默认音效必须真的在目录里：改名 / 删文件会让"开箱即响"静默失效。
        assert!(
            wavs.iter().any(|e| e.stem == "mouseclick1"),
            "出厂默认音效 mouseclick1 不在 {dir:?} 里，五键声音的默认值会指向空气"
        );
        for e in &wavs {
            let n = &e.stem;
            let d = raw(n).unwrap_or_else(|| panic!("{n} 载入失败"));
            assert!(has(n), "{n} 应被 has() 认得");
            assert_eq!(&d[0..4], b"RIFF", "{n} 不是 RIFF");
            assert_eq!(&d[8..12], b"WAVE", "{n} 不是 WAVE");
            assert_eq!(bits_per_sample(d), Some(16), "{n} 不是 16-bit PCM");
        }
    }

    /// ★ 收 Windows 原生就能播的格式；**系统解不了的连扫都不扫**（用户口径：
    /// "没必要强行读取，只要读取 Windows 能直接播放的就好"）。
    #[test]
    fn list_audio_covers_windows_playable_formats_only() {
        let dir = temp_dir("common");
        for f in [
            "a.wav", "b.MP3", "c.wma", "d.m4a", "e.aac", "f.aiff", "g.mid",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        // 系统没有解码器 / 明确不收的小众格式：**不进列表**（连看都不看，避免死选项）
        for f in ["x.wv", "y.flac", "z.ogg", "w.ape", "v.opus", "u.amr"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        // 非音频：忽略
        std::fs::write(dir.join("readme.md"), b"x").unwrap();
        std::fs::write(dir.join("noext"), b"x").unwrap();
        // 目录：忽略
        std::fs::create_dir_all(dir.join("sub.wav")).unwrap();

        let got: Vec<(String, String)> = list_audio(&dir)
            .into_iter()
            .map(|e| (e.stem, e.ext))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a".to_string(), "wav".to_string()),
                ("b".to_string(), "mp3".to_string()), // 扩展名大小写不敏感
                ("c".to_string(), "wma".to_string()),
                ("d".to_string(), "m4a".to_string()),
                ("e".to_string(), "aac".to_string()),
                ("f".to_string(), "aiff".to_string()),
                ("g".to_string(), "mid".to_string()),
            ],
            "只列 Windows 原生能播的格式，且按 stem 字典序"
        );
        assert!(list_audio(&dir.join("nope")).is_empty(), "目录不存在 → 空");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 收的格式必须**全在** `SUPPORTED_EXT` 里、且这张表不含已知解不了的格式。
    ///
    /// 这是防"手滑把 `.wv` 加回去"的护栏：一旦加回，用户就会在下拉里看到一个
    /// 永远放不出声的选项（本轮用户明确否掉的方案）。
    #[test]
    fn supported_ext_has_no_known_undecodable_format() {
        for bad in ["wv", "ape", "flac", "ogg", "oga", "opus", "amr", "caf"] {
            assert!(
                !SUPPORTED_EXT.contains(&bad),
                "「{bad}」系统没解码器，不该收（用户明确要求不强行读取小众格式）"
            );
        }
        // 反过来：真·原生格式必须在
        for good in ["wav", "mp3"] {
            assert!(SUPPORTED_EXT.contains(&good), "{good} 必须支持");
        }
        // 表内无重复（`ext_rank` 靠下标，重复会让优先级失去意义）
        let mut sorted = SUPPORTED_EXT.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SUPPORTED_EXT.len(), "SUPPORTED_EXT 有重复项");
    }

    /// 同名不同后缀只留优先级最高的那个（否则下拉出现两项、解析到同一个 id）。
    #[test]
    fn same_stem_different_ext_keeps_highest_priority() {
        let dir = temp_dir("dup");
        std::fs::write(dir.join("boom.mp3"), b"x").unwrap();
        std::fs::write(dir.join("boom.wav"), synth_wav(&[1])).unwrap();
        let got = list_audio(&dir);
        assert_eq!(got.len(), 1, "同名只留一项：{got:?}");
        assert_eq!(got[0].ext, "wav", "wav 在 SUPPORTED_EXT 里更靠前，优先");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录里放着 `.wv`（用户真实场景）：**不进列表**，而不是"进列表但标记为不可播"。
    #[test]
    fn exotic_file_in_the_dir_is_simply_absent() {
        let dir = temp_dir("exotic");
        std::fs::write(dir.join("foley_body_impact-01.wv"), b"wvpk garbage").unwrap();
        std::fs::write(dir.join("ok.wav"), synth_wav(&[1])).unwrap();
        let got = list_audio(&dir);
        assert_eq!(
            got.iter().map(|e| e.stem.as_str()).collect::<Vec<_>>(),
            vec!["ok"],
            "只有 wav 进列表；.wv 连扫都不扫"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// stem 只存文件名、不存后缀，所以解析要按扩展名挨个试。
    #[test]
    fn path_in_resolves_by_extension() {
        let dir = temp_dir("resolve");
        std::fs::write(dir.join("snd.mp3"), b"x").unwrap();
        assert_eq!(
            path_in(&dir, "snd"),
            Some(dir.join("snd.mp3")),
            "只有 mp3 时也能按 stem 找到"
        );
        // 补一个 wav：优先级更高
        std::fs::write(dir.join("snd.wav"), b"x").unwrap();
        assert_eq!(path_in(&dir, "snd"), Some(dir.join("snd.wav")));
        assert_eq!(path_in(&dir, "没有这个"), None);
        assert_eq!(path_in(&dir, ""), None, "空名不该命中任何文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_silently_absent() {
        assert!(!has("no_such_sound"));
        assert!(raw("no_such_sound").is_none());
        assert!(raw("").is_none());
    }

    #[test]
    fn real_assets_have_data_chunk() {
        // 真实文件在 fmt 与 data 之间夹了 LIST/INFO，块链必须能跳过它。
        let d = raw("click1").expect("click1");
        let (off, len) = find_chunk(d, b"data").expect("data 块");
        assert!(len > 0);
        assert_eq!(off + len, d.len(), "data 块应一直延伸到文件尾");
    }

    #[test]
    fn scale_halves_samples() {
        let wav = synth_wav(&[1000, -1000, 32767, -32768, 3]);
        let out = scale_wav(&wav, 50).expect("缩放");
        let (off, len) = find_chunk(&out, b"data").expect("data");
        assert_eq!(len, 10);
        let got: Vec<i16> = (0..5)
            .map(|i| i16::from_le_bytes([out[off + i * 2], out[off + i * 2 + 1]]))
            .collect();
        assert_eq!(got, vec![500, -500, 16384, -16384, 2]); // 32767*0.5 → 16384（round）
                                                            // 头部以外的字节原样保留
        assert_eq!(&out[..off], &wav[..off]);
    }

    #[test]
    fn scale_full_and_zero() {
        let wav = synth_wav(&[1234, -4321]);
        let full = scale_wav(&wav, 100).expect("100%");
        assert_eq!(full, wav, "100% 应逐字节不变");
        let zero = scale_wav(&wav, 0).expect("0%");
        let (off, _) = find_chunk(&zero, b"data").expect("data");
        assert_eq!(i16::from_le_bytes([zero[off], zero[off + 1]]), 0);
    }

    #[test]
    fn non_pcm_is_rejected() {
        let mut wav = synth_wav(&[1, 2, 3]);
        // 位深改成 8 → 不该被缩放
        let (fmt_off, _) = find_chunk(&wav, b"fmt ").expect("fmt");
        wav[fmt_off + 14..fmt_off + 16].copy_from_slice(&8u16.to_le_bytes());
        assert!(scale_wav(&wav, 50).is_none());
    }

    #[test]
    fn play_rejects_unknown_and_silent() {
        // 不实际发声：未知名称与 0 音量都在触到后端之前返回。
        assert!(!play("no_such_sound", 100));
        assert!(!play("click1", 0));
        assert!(!has("no_such_sound"));
        assert!(has("mouseclick1"));
    }

    #[test]
    fn mci_command_strings_are_built_and_reported() {
        // 顺带验证 MCI 通道本身是活的：`close` 一个不存在的别名不算错（MCI 幂等）。
        // 只断言"能发命令并拿到返回码"，不实际播放（CI / 无声卡也不该红）。
        let _ = mci_send(&format!("close {MCI_ALIAS}"));
        // 不存在的设备 → 必须报错（证明我们读到了错误码，而不是把失败当成功）
        assert!(mci_send("status 不存在的别名 length").is_err());
    }

    #[test]
    fn scaled_is_cached_and_stable() {
        // 同一 (名称, 档) 两次取到同一地址 —— 缓存生效，且异步播放期间指针不会失效。
        let a = scaled("click2", 30).expect("30%");
        let b = scaled("click2", 30).expect("30%");
        assert!(std::ptr::eq(a.as_ptr(), b.as_ptr()));
        // 不同档位是不同的样本
        let c = scaled("click2", 70).expect("70%");
        assert!(!std::ptr::eq(a.as_ptr(), c.as_ptr()));
    }
}
