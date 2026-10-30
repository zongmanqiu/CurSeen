//! ZIP 解压（m32）：脚本的 `ext:unzip` 用它。**只认 stored(0) / deflate(8)** 两种方法。
//!
//! # 为什么自己解析容器
//!
//! 引入整包 `zip` crate 会再牵一串依赖；而 ZIP 的容器结构（EOCD → 中央目录 → 本地头）
//! 是几十行能说清的东西，DEFLATE 本身则由 `flate2` 提供——它**早就在产物里**（`png` 的解码
//! 链），所以显式声明它不增加体积。少一个 crate、多 200 行我们看得懂的代码。
//!
//! # 安全面（这不是沙箱，是防手滑）
//!
//! 脚本的信任模型是"装脚本 = 交出账号权限"（API.md），所以这里**不做**目录白名单；
//! 但**路径逃逸必须拒**——`../`、绝对路径、盘符、`\` 分隔、以及 **符号链接**条目。
//! 否则一个排版正常的"素材包"能把文件写到目标目录之外，用户连发生了什么都看不出来。
//! 同理，CRC 不符、尺寸超限、加密条目都**拒**而不是"尽量解"。
//!
//! # 上限
//!
//! · 单个条目解压后 ≤ [`MAX_ENTRY_UNCOMPRESSED`]；整个包 ≤ [`MAX_TOTAL_UNCOMPRESSED`]。
//! · ZIP64（条目数/偏移是 0xFFFFFFFF 哨兵）不支持，直接给一句原因。
//! · 文件名按 UTF-8 宽松解：老 zip 里 GBK 编码的名字会变成替换字符（`�`）。
//!   这是"能解出来但名字难看"，不影响内容——要精确还得猜码页，不值得在这里做。

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// 单个条目解压后的上限（256 MiB）。
pub const MAX_ENTRY_UNCOMPRESSED: u64 = 256 * 1024 * 1024;
/// 整包解压后的总上限（2 GiB）。
pub const MAX_TOTAL_UNCOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;

const EOCD_SIG: u32 = 0x0605_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
/// EOCD 最多再跟 64 KiB 注释，所以从文件尾往前找这么远就够。
const EOCD_SCAN: u64 = 64 * 1024 + 22;

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    comp: u64,
    uncomp: u64,
    local: u64,
    ext_attr: u32,
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 解压 `zip` 到 `dest`（目录不存在就建）。返回写出的**文件**数（目录不算）。
///
/// 同名的已有文件会被覆盖（与"解压一个包"的常识一致）；写文件先写 `.partial` 再改名，
/// 任何失败都不留半成品。
pub fn extract_zip(zip: &Path, dest: &Path) -> Result<u32, String> {
    let mut f =
        std::fs::File::open(zip).map_err(|e| format!("打不开压缩包（{}）：{e}", zip.display()))?;
    let len = f
        .metadata()
        .map_err(|e| format!("读不到压缩包大小：{e}"))?
        .len();
    if len < 22 {
        return Err("这不是一个 ZIP：文件比最小结构还短".to_string());
    }
    // —— 找 EOCD（从尾部往前扫）——
    let tail_len = len.min(EOCD_SCAN);
    let mut tail = vec![0u8; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len))
        .map_err(|e| format!("读压缩包失败：{e}"))?;
    f.read_exact(&mut tail)
        .map_err(|e| format!("读压缩包失败：{e}"))?;
    let mut eocd = None;
    for i in (0..tail.len().saturating_sub(21)).rev() {
        if u32_at(&tail, i) == Some(EOCD_SIG) {
            eocd = Some(i);
            break;
        }
    }
    let Some(eocd) = eocd else {
        return Err("这不是一个 ZIP：找不到目录尾（EOCD）".to_string());
    };
    let count = u16_at(&tail, eocd + 10).unwrap_or(0) as u64;
    let cd_size = u32_at(&tail, eocd + 12).unwrap_or(0) as u64;
    let cd_off = u32_at(&tail, eocd + 16).unwrap_or(0) as u64;
    if count == 0xFFFF || cd_size == u32::MAX as u64 || cd_off == u32::MAX as u64 {
        return Err("不支持 ZIP64（条目数或偏移超出 32 位）".to_string());
    }
    if cd_off + cd_size > len {
        return Err("压缩包的目录区越界（文件被截断？）".to_string());
    }
    // —— 中央目录 ——
    let mut cd = vec![0u8; cd_size as usize];
    f.seek(SeekFrom::Start(cd_off))
        .map_err(|e| format!("读目录区失败：{e}"))?;
    f.read_exact(&mut cd)
        .map_err(|e| format!("读目录区失败：{e}"))?;
    let mut entries = Vec::new();
    let mut p = 0usize;
    for _ in 0..count {
        if u32_at(&cd, p) != Some(CENTRAL_SIG) {
            return Err("压缩包目录项损坏（签名不对）".to_string());
        }
        let flags = u16_at(&cd, p + 8).unwrap_or(0);
        let method = u16_at(&cd, p + 10).unwrap_or(0);
        let crc = u32_at(&cd, p + 16).unwrap_or(0);
        let comp = u32_at(&cd, p + 20).unwrap_or(0) as u64;
        let uncomp = u32_at(&cd, p + 24).unwrap_or(0) as u64;
        let name_len = u16_at(&cd, p + 28).unwrap_or(0) as usize;
        let extra_len = u16_at(&cd, p + 30).unwrap_or(0) as usize;
        let comment_len = u16_at(&cd, p + 32).unwrap_or(0) as usize;
        let ext_attr = u32_at(&cd, p + 38).unwrap_or(0);
        let local = u32_at(&cd, p + 42).unwrap_or(0) as u64;
        let name_b = cd
            .get(p + 46..p + 46 + name_len)
            .ok_or_else(|| "压缩包目录项越界".to_string())?;
        let name = String::from_utf8_lossy(name_b).into_owned();
        if flags & 0x1 != 0 {
            return Err(format!("条目「{name}」是加密的，不支持"));
        }
        if comp == u32::MAX as u64 || uncomp == u32::MAX as u64 || local == u32::MAX as u64 {
            return Err(format!("条目「{name}」需要 ZIP64，不支持"));
        }
        entries.push(Entry {
            name,
            method,
            crc,
            comp,
            uncomp,
            local,
            ext_attr,
        });
        p += 46 + name_len + extra_len + comment_len;
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("建目标目录失败（{}）：{e}", dest.display()))?;

    let mut files = 0u32;
    let mut total = 0u64;
    for e in entries {
        // —— 路径逃逸 / 符号链接：一律拒（防手滑，不是沙箱）——
        let (rel, is_dir) = safe_relative(&e.name)?;
        if is_dir {
            std::fs::create_dir_all(dest.join(&rel))
                .map_err(|err| format!("建子目录失败（{}）：{err}", rel.display()))?;
            continue;
        }
        let mode = (e.ext_attr >> 16) & 0xF000;
        if mode == 0xA000 {
            return Err(format!("条目「{}」是符号链接，拒绝解压", e.name));
        }
        total = total.saturating_add(e.uncomp);
        if e.uncomp > MAX_ENTRY_UNCOMPRESSED {
            return Err(format!(
                "条目「{}」解压后有 {} MB，超过单条目上限 {} MB",
                e.name,
                e.uncomp / (1024 * 1024),
                MAX_ENTRY_UNCOMPRESSED / (1024 * 1024)
            ));
        }
        if total > MAX_TOTAL_UNCOMPRESSED {
            return Err(format!(
                "整包解压后会超过 {} MB 上限，拒绝",
                MAX_TOTAL_UNCOMPRESSED / (1024 * 1024)
            ));
        }
        let data = read_entry_data(&mut f, &e)?;
        // ——— 本地头 → 压缩数据 ———
        let out = match e.method {
            0 => data,
            8 => {
                let mut z = flate2::read::DeflateDecoder::new(&data[..]);
                let mut out = Vec::with_capacity(e.uncomp.min(1 << 22) as usize);
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    let n = z
                        .read(&mut chunk)
                        .map_err(|err| format!("条目「{}」解压失败：{err}", e.name))?;
                    if n == 0 {
                        break;
                    }
                    out.extend_from_slice(&chunk[..n]);
                    if out.len() as u64 > e.uncomp.max(1) {
                        return Err(format!("条目「{}」解压后比声明的更大（损坏？）", e.name));
                    }
                }
                out
            }
            m => {
                return Err(format!(
                    "条目「{}」用了不支持的压缩方法 {m}（只认 stored/deflate）",
                    e.name
                ))
            }
        };
        if out.len() as u64 != e.uncomp {
            return Err(format!(
                "条目「{}」解压后 {} 字节 / 声明 {} 字节，对不上",
                e.name,
                out.len(),
                e.uncomp
            ));
        }
        let mut crc = flate2::Crc::new();
        crc.update(&out);
        if crc.sum() != e.crc {
            return Err(format!("条目「{}」CRC 不符（压缩包损坏或被改过）", e.name));
        }
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("建子目录失败（{}）：{err}", parent.display()))?;
        }
        let partial = out_path.with_extension("partial");
        std::fs::write(&partial, &out)
            .map_err(|err| format!("写文件失败（{}）：{err}", rel.display()))?;
        std::fs::rename(&partial, &out_path).map_err(|err| {
            let _ = std::fs::remove_file(&partial);
            format!("落盘失败（{}）：{err}", rel.display())
        })?;
        files += 1;
    }
    Ok(files)
}

/// 定位到条目数据起点并读出压缩字节。
fn read_entry_data(f: &mut std::fs::File, e: &Entry) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(e.local))
        .map_err(|err| format!("定位条目「{}」失败：{err}", e.name))?;
    let mut hdr = [0u8; 30];
    f.read_exact(&mut hdr)
        .map_err(|_| format!("条目「{}」的本地头读不全", e.name))?;
    if u32_at(&hdr, 0) != Some(LOCAL_SIG) {
        return Err(format!("条目「{}」的本地头签名不对", e.name));
    }
    let name_len = u16_at(&hdr, 26).unwrap_or(0) as u64;
    let extra_len = u16_at(&hdr, 28).unwrap_or(0) as u64;
    let data_off = e.local + 30 + name_len + extra_len;
    f.seek(SeekFrom::Start(data_off))
        .map_err(|err| format!("定位条目「{}」数据失败：{err}", e.name))?;
    let mut data = vec![0u8; e.comp as usize];
    f.read_exact(&mut data)
        .map_err(|_| format!("条目「{}」的数据读不全（文件被截断？）", e.name))?;
    Ok(data)
}

/// 条目名 → 安全相对路径。**任何可能写到目标目录之外的形状都拒**。
fn safe_relative(name: &str) -> Result<(PathBuf, bool), String> {
    let is_dir = name.ends_with('/') || name.ends_with('\\');
    if name.is_empty() {
        return Err("压缩包里有空名字的条目".to_string());
    }
    let bytes = name.as_bytes();
    if bytes[0] == b'/' || bytes[0] == b'\\' {
        return Err(format!("条目「{name}」是绝对路径，拒绝解压"));
    }
    let mut out = PathBuf::new();
    for comp in name.split(['/', '\\']) {
        if comp.is_empty() || comp == "." {
            continue; // 目录名的尾斜杠 / "a/./b"
        }
        if comp == ".." {
            return Err(format!("条目「{name}」含有 `..`，会写到目标目录之外，拒绝"));
        }
        if comp.contains(':') {
            return Err(format!("条目「{name}」的成分里带冒号（盘符/数据流），拒绝"));
        }
        out.push(comp);
    }
    if out.as_os_str().is_empty() {
        return Err(format!("条目「{name}」解析后没有有效路径"));
    }
    Ok((out, is_dir))
}

#[cfg(test)]
/// 造一个 ZIP：`method` 0=stored、8=deflate。名字按原样写（攻击样本要用）。
pub(crate) fn make_zip(entries: &[(&str, &[u8], u16)]) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, method) in entries {
        let crc = {
            let mut c = flate2::Crc::new();
            c.update(data);
            c.sum()
        };
        let payload: Vec<u8> = if *method == 8 {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        } else {
            data.to_vec()
        };
        let local_off = out.len() as u32;
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&payload);
        central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by (DOS)
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // int attr
        central.extend_from_slice(&0u32.to_le_bytes()); // ext attr（普通文件）
        central.extend_from_slice(&local_off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("curseen_unzip_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// ★ 两种方法（stored / deflate）都要能解、字节一致、目录条目被建出来。
    #[test]
    fn extracts_stored_and_deflated_entries_byte_exact() {
        let text = b"hello zip \xe4\xbd\xa0\xe5\xa5\xbd".repeat(50);
        let zip = make_zip(&[
            ("a.txt", b"stored-body", 0),
            ("dir/", b"", 0),
            ("dir/b.txt", &text, 8),
        ]);
        let d = tmpdir("ok");
        let z = d.join("t.zip");
        std::fs::write(&z, &zip).unwrap();
        let out = d.join("out");
        let n = extract_zip(&z, &out).expect("该解得开");
        assert_eq!(n, 2, "只数文件，目录不算");
        assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"stored-body");
        assert_eq!(std::fs::read(out.join("dir/b.txt")).unwrap(), text);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 路径逃逸（`..` / 绝对路径 / 盘符）与符号链接一律拒——**包括 reject 之后没写出文件**。
    /// 这是本模块的安全面里唯一"必须有"的一组格。
    #[test]
    fn refuses_escaping_and_absolute_and_symlink_entries() {
        for bad in [
            "../evil.txt",
            "a/../../evil.txt",
            "/abs.txt",
            "C:/evil.txt",
            "x\\..\\evil.txt",
        ] {
            let zip = make_zip(&[(bad, b"x", 0)]);
            let d = tmpdir("bad");
            let z = d.join("t.zip");
            std::fs::write(&z, &zip).unwrap();
            let out = d.join("out");
            let e = extract_zip(&z, &out).unwrap_err();
            assert!(
                e.contains("拒绝") || e.contains("..") || e.contains("绝对"),
                "「{bad}」该被拒，实际：{e}"
            );
            assert!(!d.join("evil.txt").exists() && !d.join("abs.txt").exists());
            let _ = std::fs::remove_dir_all(&d);
        }
        // 符号链接：ext_attr 高 16 位是 unix mode，0xA1FF = 0777 的 symlink
        let mut zip = make_zip(&[("link.txt", b"/etc/passwd", 0)]);
        // 把中央目录里 ext_attr 改成 symlink（定位方式：在中央目录项里找 ext_attr 字段）
        let cd = zip
            .windows(4)
            .position(|w| w == CENTRAL_SIG.to_le_bytes())
            .unwrap();
        let ext_off = cd + 38;
        zip[ext_off..ext_off + 4].copy_from_slice(&0xA1FF_0000u32.to_le_bytes());
        let d = tmpdir("sym");
        let z = d.join("t.zip");
        std::fs::write(&z, &zip).unwrap();
        let e = extract_zip(&z, &d.join("out")).unwrap_err();
        assert!(e.contains("符号链接"), "实际：{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 覆盖解压：同一个包解到**已经有同名文件**的目录（升级素材包的常见动作）。
    /// Windows 的 `fs::rename` 在目标已存在时会失败（MoveFileEx 没给 REPLACE），
    /// 所以这条不是"锦上添花"——它是解压第二次就开始红的形状。
    #[test]
    fn extracts_over_an_existing_file() {
        let zip = make_zip(&[("a.txt", b"v2", 0)]);
        let d = tmpdir("over");
        let z = d.join("t.zip");
        std::fs::write(&z, &zip).unwrap();
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("a.txt"), b"v1-old").unwrap();
        let n = extract_zip(&z, &out).expect("覆盖解压必须成功");
        assert_eq!(n, 1);
        assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 数据被改一个字节 ⇒ CRC 不符要**拒**（而不是把坏文件写出去）。
    #[test]
    fn corrupt_payload_is_refused_by_crc() {
        let mut zip = make_zip(&[("a.txt", b"0123456789", 0)]);
        // 在本地头的名字之后找数据起点：30 + 5 字节名
        let data_at = 30 + "a.txt".len();
        zip[data_at] ^= 0xFF;
        let d = tmpdir("crc");
        let z = d.join("t.zip");
        std::fs::write(&z, &zip).unwrap();
        let out = d.join("out");
        let e = extract_zip(&z, &out).unwrap_err();
        assert!(e.contains("CRC"), "实际：{e}");
        assert!(!out.join("a.txt").exists(), "坏文件不许落盘");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 互操作：解一个**别的工具产出的真 zip**（`tests/fixtures/sample_py.zip`，Python 的
    /// `zipfile` 写、默认 deflate）。自家 builder 写、自家 reader 读会共享同一套假设；
    /// 这一格才是"现实里的包能不能解"的读数。
    #[test]
    fn extracts_a_zip_produced_by_another_tool() {
        let zip =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_py.zip");
        let d = tmpdir("foreign");
        let out = d.join("out");
        let n = extract_zip(&zip, &out).expect("该解得开别的工具写的包");
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(out.join("hello.txt")).expect("文本文件");
        assert!(
            text.starts_with("你好，ZIP"),
            "中文内容要一字不差：{text:?}"
        );
        let bin = std::fs::read(out.join("sub/data.bin")).expect("二进制文件");
        assert_eq!(bin, (0..=255u8).cycle().take(1024).collect::<Vec<u8>>());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 不是 ZIP / 空文件 ⇒ 一句原因，不 panic。
    #[test]
    fn non_zip_inputs_get_a_reason() {
        let d = tmpdir("junk");
        let z = d.join("t.zip");
        std::fs::write(&z, b"not a zip at all, just text").unwrap();
        let e = extract_zip(&z, &d.join("out")).unwrap_err();
        assert!(e.contains("ZIP"), "实际：{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
