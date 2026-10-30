//! 文档一致性门禁（对齐 Pecia 的 `test_docs`）。
//!
//! 为什么要有这一文件：PR 里最常见的文档腐化是"代码挪了，说明还留在原处"。靠自觉核对
//! 不可靠——这里是让持续集成替我们记住这件事。它只查**事实**（文件在不在、提到了没有），
//! 不查文笔，所以误报率低、值得常驻。
//!
//! 覆盖四条事实：
//! 1. `DOCS_MANIFEST.md` / `AGENTS.md` / `目录结构说明.md` /
//!    `HISTORY.md` / `docs/DESIGN.md` / `patches/README.md` / `build/build_readme.md`
//!    这些受门禁约束的文件都存在且非空；
//! 2. `main/LICENSE` 是**逐字**的 GNU AGPL-3.0 全文，且是 UTF-8（见
//!    `test_license_is_verbatim_agpl3_and_utf8`）——许可文本被改写或存成 UTF-16
//!    都是分发期才暴露的问题，这里提前挡住；
//! 3. `目录结构说明.md` 提到了 `src/` 下每一个顶层模块与关键文件——漏一个即代表有人
//!    加了模块却没写进结构说明；
//! 4. `.3rdParty/wind-ui-rust-*` 里每一处 `CurSeen 补丁 0NN` 标记，都能在
//!    `patches/**/PATCHES.md` 找到对应小节——防止"改了第三方却没登记"，那是升级
//!    依赖时最容易丢的东西。（★ 发现式扫描：不写死版本号，0.16.1→0.19.0→0.20.0→0.30.0
//!    三次迁移都因为写死路径而扫错过地方。）
//!
//! 5. 现行文档只写"现在是什么"与"该怎么做"，不带"考古叙述"
//!    （`★ 2026-09-27 核查：…已不存在` 这类日期戳式审计记录，或指向
//!    `temp/notes-archive/` 这类不存在路径的死引用）。判据见《开发指南.md》§七。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // tests/ 在 main/ 下，项目根是 main 的上一级。
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("main 的上一级即项目根")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path:?} 失败：{e}"))
}

#[test]
fn test_docs_checks_the_manifest_files_exist() {
    let root = repo_root();
    let listed = [
        ("main/DOCS_MANIFEST.md", "文档清单"),
        ("main/AGENTS.md", "项目规则"),
        ("main/目录结构说明.md", "结构说明"),
        ("main/开发指南.md", "开发指南"),
        ("main/HISTORY.md", "版本历史"),
        ("main/THIRD-PARTY-NOTICES.md", "第三方许可"),
        ("main/docs/DESIGN.md", "设计指引"),
        ("main/patches/README.md", "补丁目录说明"),
        ("main/build/build_readme.md", "构建说明"),
    ];
    for (rel, why) in listed {
        let p = root.join(rel);
        assert!(p.is_file(), "{why}缺失：{rel}");
        let body = read(&p);
        assert!(
            !body.trim().is_empty(),
            "{why}为空：{rel}——空文档比没文档更糟，它会在核对时给出'已检查'的错觉"
        );
        assert!(body.lines().count() > 10, "{why}内容过短：{rel}");
    }
}

/// AGPL-3.0 第 11 条（专利）的官方措辞。
///
/// 为什么单独钉这一句：仓库里曾有一份"手抄"的 AGPL 文本把它写成
/// `you convey, or obtain by conveying`，少了 `or propagate by procuring conveyance of`
/// 这半句——整份许可看着天衣无缝，只有这一处是实质改写。用官方原文里最长的这段
/// 当指纹，任何删改/转录错误都会在这里现形。
const AGPL3_S11_FINGERPRINT: &str =
    "you convey, or propagate by procuring conveyance of, a covered work";

#[test]
fn test_license_is_verbatim_agpl3_and_utf8() {
    let root = repo_root();
    let p = root.join("main/LICENSE");
    assert!(
        p.is_file(),
        "缺少 main/LICENSE——AGPL 要求随二进制分发许可全文"
    );

    // `read_to_string` 只接受合法 UTF-8：UTF-16（PowerShell 重定向的默认产物）
    // 会直接在这里炸掉，这正是我们要的判据。
    let body = std::fs::read_to_string(&p).expect("main/LICENSE 不是合法 UTF-8");
    let raw = std::fs::read(&p).expect("读 main/LICENSE 失败");
    assert!(
        !raw.starts_with(&[0xEF, 0xBB, 0xBF]),
        "main/LICENSE 带 UTF-8 BOM——许可文本应保持无 BOM 的纯文本"
    );

    // 折行归一化：官方文本是硬折行的纯文本，一句话常被劈到两行上。
    // 压成单空格后再比对，判据就不受排版/换行风格影响。
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "GNU AFFERO GENERAL PUBLIC LICENSE",
        "Version 3, 19 November 2007",
        "END OF TERMS AND CONDITIONS",
        AGPL3_S11_FINGERPRINT,
    ] {
        assert!(
            flat.contains(needle),
            "main/LICENSE 缺少官方 AGPL-3.0 的锚点 {needle:?}——\
             说明这份许可被改写或不是 v3 全文"
        );
    }
    assert!(
        !flat.contains("or obtain by conveying"),
        "main/LICENSE 含手抄版 AGPL 第 11 条的错误措辞——请从 \
         <https://www.gnu.org/licenses/agpl-3.0.txt> 重新取全文"
    );

    // 逐字 = 一个字都不许多。这个文件必须**以官方标题行开头**，且不含本项目
    // 的任何痕迹——版权声明放在 README 与 Cargo.toml 的 authors，不进 LICENSE。
    // 一旦允许"只加一段在开头"，比对就退化为模糊匹配，官方出新版也无法直接覆盖。
    assert!(
        body.starts_with("                    GNU AFFERO GENERAL PUBLIC LICENSE\n"),
        "main/LICENSE 首行不是官方标题行——许可文本必须逐字，\
         开头不允许加版权块或任何说明（版权声明见 README/Cargo.toml）"
    );
    for forbidden in ["CurSeen", "qiuzongman", "邱宗满"] {
        assert!(
            !body.contains(forbidden),
            "main/LICENSE 里出现了 {forbidden:?}——本项目的署名不应写进许可文本，\
             请放到 README 的 License 章节"
        );
    }

    // 分发目录里的副本必须与正本逐字一致。这一步由 `build/gen_licenses.py` 在部署时
    // 自动完成（copy-if-different，`6_deploy.bat` 的 [c/3] 会复核），所以正常情况下
    // 两者恒等；本断言是给"有人手工编辑了 build/ 里的副本"或"部署没跑"兜底。
    let shipped = root.join("build/LICENSE");
    if shipped.is_file() {
        assert_eq!(
            std::fs::read(&shipped).expect("读 build/LICENSE 失败"),
            raw,
            "build/LICENSE 与 main/LICENSE 不一致——请重跑步骤 6（deploy.py 会调用 \
             build/gen_licenses.py 同步），不要手工编辑发布目录里的副本"
        );
    }
}

/// 许可必须**随包分发**：`build/` 是可直接拿出去分发的目录，AGPL-3.0 要求随二进制
/// 保留自身许可全文 + 全部第三方许可声明（见 THIRD-PARTY-NOTICES.md 第四节）。
///
/// 本表是 `build/gen_licenses.py` 里那张映射的**独立副本**，这是有意为之：门禁若直接
/// 读被测代码的表，就变成了"代码自己说自己对"。上游文件名不统一（windui 是
/// `LICENSE-MIT`/`LICENSE-APACHE`），所以必须逐个点名。
///
/// `build/` 是运行期目录（不进 git），没部署过的机器上它可能不存在 → 那时跳过；
/// 但只要它在，就必须是完整的。
#[test]
fn test_shipped_licence_bundle_is_complete() {
    let root = repo_root();
    let dest = root.join("build");
    let notices_dest = dest.join("licenses").join("THIRD-PARTY-NOTICES.md");
    if !notices_dest.is_file() {
        return; // 从未部署过，不做判据
    }

    // 本文件必须与正本逐字一致（正本是单一来源）。
    // 用 assert! + 长度对比而不是 assert_eq!：后者失败时会把两份文档的字节全 dump 出来，
    // 几万字节刷屏，反而看不见"到底哪个文件不一致"这句关键信息。
    let shipped_notices =
        std::fs::read(&notices_dest).expect("读发布版 THIRD-PARTY-NOTICES.md 失败");
    let canon_notices = std::fs::read(root.join("main/THIRD-PARTY-NOTICES.md"))
        .expect("读正本 THIRD-PARTY-NOTICES.md 失败");
    assert!(
        shipped_notices == canon_notices,
        "build/licenses/THIRD-PARTY-NOTICES.md 与 main/ 的正本不一致（发布版 {} B，正本 {} B）——\
         请重跑步骤 6（python main/build/gen_licenses.py 单独补也行）",
        shipped_notices.len(),
        canon_notices.len()
    );

    // 每个本地 path 依赖的许可原文都要在。缺一个 = 分发漏了署名义务。
    let lic = dest.join("licenses");
    let expected = [
        (
            "windui-LICENSE-MIT.txt",
            ".3rdParty/wind-ui-rust-0.30.0/LICENSE-MIT",
        ),
        (
            "windui-LICENSE-APACHE.txt",
            ".3rdParty/wind-ui-rust-0.30.0/LICENSE-APACHE",
        ),
    ];
    for (shipped_name, src_rel) in expected {
        let p = lic.join(shipped_name);
        assert!(
            p.is_file(),
            "缺少 build/licenses/{shipped_name}——发布目录缺许可原文，\
             请跑步骤 6（或用 python main/build/gen_licenses.py 单独补）"
        );
        let body = read(&p);
        assert!(
            body.lines().count() > 3,
            "build/licenses/{shipped_name} 只有 {} 行，疑似被截断",
            body.lines().count()
        );
        // 源在（.3rdParty 已就位）时就逐字比对；源被清空则跳过比对（仍要求上面两条成立）。
        let src = root.join(src_rel);
        if src.is_file() {
            let from_build = std::fs::read(&p).expect("读发布版许可失败");
            let from_src = std::fs::read(&src).expect("读 .3rdParty 许可源失败");
            assert!(
                from_build == from_src,
                "build/licenses/{shipped_name} 与 {src_rel} 不一致\
                 （{} B vs {} B）——许可文本必须逐字，不允许任何改写",
                from_build.len(),
                from_src.len()
            );
        }
    }
}

/// 构建脚本的一个已实测陷阱：**`echo` 行里的半角括号必须转义成 `^(` / `^)`**。
///
/// cmd.exe 不会把 `( ... )` 块里 `echo` 文本中的括号当成"嵌套"——第一个 `)` 就把块
/// 提前关掉了（实测见 `temp/paren_probe*.bat`）：
///
/// ```text
/// if 1==1 ( echo foo (bar) baz )   -> "baz was unexpected at this time"（同一行还有字）
/// if 1==1 ( echo foo (bar) )       -> 静默：块在 ")" 处结束，块内后续语句变成无条件执行
/// ```
///
/// 后者尤其阴——`full.bat` 里 `echo pipeline stopped at step 1 (toolchain)` 会把块内
/// 紧跟的 `pause` / `exit /b 1` 变成**无条件**执行，于是流水线每次都停在第 1 步。
/// 症状是"看起来跑通了但其实啥也没干"，靠肉眼极难发现，所以钉成门禁。
#[test]
fn test_build_batch_echo_lines_escape_parentheses() {
    let root = repo_root();
    let build = root.join("main/build");
    let mut bat: Vec<PathBuf> = std::fs::read_dir(&build)
        .expect("读 main/build 目录失败")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("bat"))
        .collect();
    bat.sort();
    assert!(!bat.is_empty(), "main/build/ 下应有 .bat 脚本");

    let mut bad: Vec<String> = Vec::new();
    for p in &bat {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        for (i, line) in read(p).lines().enumerate() {
            let t = line.trim_start();
            if t.to_ascii_uppercase().starts_with("REM") {
                continue; // 注释行不经过 cmd 的 echo 解析
            }
            if !t.to_ascii_lowercase().starts_with("echo") {
                continue;
            }
            // 逐个字符找"没有被 ^ 转义"的括号。
            let bytes = t.as_bytes();
            for (j, &c) in bytes.iter().enumerate() {
                if c != b'(' && c != b')' {
                    continue;
                }
                let escaped = j > 0 && bytes[j - 1] == b'^';
                if !escaped {
                    bad.push(format!("{name}:{}: {}", i + 1, line.trim()));
                    break;
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "以下 echo 行含未转义的半角括号——请写成 ^( 和 ^)：\n  {}\n\
         cmd.exe 会在第一个 ) 处提前关闭 if/for 块：同行还有字则直接语法错，\
         行尾则静默把块内后续语句变成无条件执行。",
        bad.join("\n  ")
    );
}

/// `main/build/deps.txt` 是第三方源码的**下载登记表**（`2_download.bat` 逐行消费）。
///
/// 两条硬约束，都是实测踩过的：
/// 1. **必须 ASCII-only** —— 文件由 `cmd.exe` 按 OEM 码页解析（本机 GBK），
///    一旦写进非 ASCII（中文注释最容易）就会乱码，`for /f` 拆出来的 url/marker 全错；
/// 2. **每行必须是 `name=url|marker`** —— `name` 要和 `.3rdParty/<name>/` 目录名以及
///    `Cargo.toml` 的 path 依赖对上，否则下载下来也没人用。
#[test]
fn test_deps_download_manifest_is_ascii_and_well_formed() {
    let root = repo_root();
    let p = root.join("main/build/deps.txt");
    assert!(
        p.is_file(),
        "缺少 main/build/deps.txt——从头构建时无人知道去哪下依赖"
    );

    let raw = std::fs::read(&p).expect("读 deps.txt 失败");
    assert!(
        raw.is_ascii(),
        "deps.txt 必须 ASCII-only：cmd.exe 按 OEM 码页解析它，非 ASCII 会乱码成错的 url/marker"
    );
    let body = String::from_utf8(raw).expect("ASCII 即合法 UTF-8");
    let cargo = read(&root.join("main/Cargo.toml"));

    let mut entries = 0usize;
    for (i, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, rest) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("deps.txt 第 {} 行缺 '='：{line}", i + 1));
        let (url, marker) = rest
            .split_once('|')
            .unwrap_or_else(|| panic!("deps.txt 第 {} 行缺 '|marker'：{line}", i + 1));
        assert!(
            url.starts_with("https://"),
            "deps.txt 第 {} 行的 url 不是 https：{url}",
            i + 1
        );
        assert!(
            !marker.trim().is_empty(),
            "deps.txt 第 {} 行的 marker 为空——它是「这次下载完整吗」的唯一探针",
            i + 1
        );
        // name 必须真的被 Cargo.toml 当作 path 依赖引用，否则就是死条目。
        assert!(
            cargo.contains(&format!(".3rdParty/{name}")),
            "deps.txt 里的 {name} 没有出现在 main/Cargo.toml 的 .3rdParty/ 路径里——\
             下载了也没人用，要么是拼错了，要么该删掉这一行"
        );
        entries += 1;
    }
    // windui 是唯一的 path 依赖（nnnoiseless 2026-09-28 起走 crates.io）。
    // 门槛设 1 而不是 0：windui 要打补丁、必须本地持有，这条下载链路不许消失。
    assert!(
        entries >= 1,
        "deps.txt 只解析出 {entries} 条——扫描逻辑或文件内容已坏（预期 >= 1，windui）"
    );
}

#[test]
fn test_structure_doc_lists_every_source_module() {
    let root = repo_root();
    let doc = read(&root.join("main/目录结构说明.md"));
    let src = root.join("main/src");

    // 顶层模块：每一个都必须在结构说明里出现，否则读者无从知道它负责什么。
    let mut missing: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&src)
        .expect("读 src 目录失败")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "src 下应有子目录");
    for d in &dirs {
        let name = d
            .file_name()
            .and_then(|n| n.to_str())
            .expect("目录名应为 UTF-8");
        if !doc.contains(name) {
            missing.push(name.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "以下 src 子目录未出现在《目录结构说明.md》：{missing:?}\n\
         ——新增模块请在该文档第二节的表格里登记职责"
    );

    // 入口文件也要提到。
    for f in ["main.rs", "lib.rs"] {
        assert!(doc.contains(f), "《目录结构说明.md》未提到入口 {f}");
    }
}

/// 全 src 树里有没有**同名**文件（只比对文件名，不看路径）。
fn src_tree_contains(src_dir: &std::path::Path, file_name: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(src_dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if src_tree_contains(&p, file_name) {
                return true;
            }
        } else if p
            .file_name()
            .map(|n| n == std::ffi::OsStr::new(file_name))
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// ★ 2026-09-27 补：增量门禁的另一半。
///
/// `test_structure_doc_lists_every_source_module` 只查"**列全**"（src 下每个模块都被提到），
/// 查不出"**列准**"——2026-09-27 删掉 `platform/keep_awake.rs` 后，《目录结构说明.md》
/// 的 platform 表格里仍写着它：文档指向一个不存在的文件，比不写更糟。
/// 本条把 §二「各目录职责」里点名的每一个 `.rs` 都对到磁盘上——**删文件必须同步删文档条目**。
#[test]
fn docs_named_source_files_must_actually_exist() {
    let root = repo_root();
    let doc = read(&root.join("main/目录结构说明.md"));

    // ① 先按**全部** `### ` 标题切出「节起点」列表（不只看 `### src/`，见下）。
    //    边界计算必须用全部标题：否则 `### tests/ — 集成测试门禁` 会被当成 `src/app/`
    //    那一节的尾巴，于是 tests 段里点名的 `tests/docs_consistency.rs` 变成误报。
    // ② 再只挑标题以 `### src/` 开头的节。
    // ③ 每节的正文 = 起点 → 下一个**任意** `### ` 标题（或文末）。
    #[derive(Debug)]
    struct Sec {
        dir: String,
        body: String,
    }
    let mut secs: Vec<Sec> = Vec::new();
    let marks: Vec<(String, usize)> = doc
        .match_indices("\n### ")
        .map(|(i, _)| {
            let rest = &doc[i + 1..];
            let eol = rest.find('\n').unwrap_or(rest.len());
            (rest[..eol].trim().to_string(), i)
        })
        .collect();
    for idx in 0..marks.len() {
        let (head, at) = &marks[idx];
        if !head.starts_with("### src/") {
            continue;
        }
        // `### src/core/ — 纯逻辑层` / `### src/app/ / src/update/ — 应用层与自动更新`
        // → 都取到 `core` / `app`：先切破折号，再切 " / "，最后去尾斜杠。
        let mut dir = head
            .trim_start_matches("### ")
            .trim()
            .split('—')
            .next()
            .unwrap_or("")
            .split(" / ")
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string();
        if let Some(rest) = dir.strip_prefix("src/") {
            dir = rest.to_string();
        }
        if dir.is_empty() {
            continue;
        }
        let next_at = marks.get(idx + 1).map(|(_, a)| *a).unwrap_or(doc.len());
        let body = doc[*at..next_at].to_string();
        secs.push(Sec { dir, body });
    }
    assert!(
        secs.len() >= 6,
        "《目录结构说明.md》里应至少分出 6 个 src 小节，实际 {secs:?}"
    );

    // 正文里点名的每个 `.rs`（按反引号/空格/括号切词）都必须在 src 树里真的存在。
    let mut stale: Vec<String> = Vec::new();
    for sec in &secs {
        for m in sec.body.split(['`', ' ', '(', ')']) {
            let Some(name) = m.trim().strip_suffix(".rs") else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            // `platform/pip_window.rs` 按它自己的路径查；`audio.rs` 按本节目录查。
            // ★ 命中不了先别急：正文引用**别节**的 `.rs` 很常见（core 表提 `runtime/render.rs`、
            //   platform 表提 `core/logger.rs`），故再回退到全 src 树按同名找。
            let fname = format!("{name}.rs");
            let cand = root.join("main/src").join(if name.contains('/') {
                fname.clone()
            } else {
                format!("{}/{}", sec.dir, fname)
            });
            if !cand.is_file() && !src_tree_contains(&root.join("main/src"), &fname) {
                stale.push(format!(
                    "`{name}.rs` 在《目录结构说明.md》的 {} 节里被点名，但 src 树中没有该文件",
                    sec.dir
                ));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "《目录结构说明.md》列出了已不存在的源码文件：\n  {}\n\
         ——删文件必须同步删掉文档里那一行（**别改成「该文件已删除」**：现行文档不写
         考古叙述，见 test_current_docs_carry_no_archaeology）",
        stale.join("\n  ")
    );
}

/// ★ 辅助：一行里出现 `★` 且其后 60 字符内含"核查" → 疑似考古日期戳。
fn looks_like_archaeology_stamp(line: &str) -> bool {
    let mut rest = line;
    while let Some(i) = rest.find('★') {
        let after: String = rest[i..].chars().take(60).collect();
        if after.contains("核查") {
            return true;
        }
        rest = &rest[i + '★'.len_utf8()..];
    }
    false
}

/// 现行文档不得携带"考古叙述"。
///
/// **判据**（《开发指南.md》§七）：现行文档只写"现在是什么"和"该怎么做"。
/// 违规的典型长相（都是这条门禁的起因，曾经在 6 份文档里互相抄）：
///   - `★ 2026-09-27 核查：temp/retired/ 该目录已不存在` —— 日期戳式审计记录；
///   - `别以为它还在` / `照旧路径去找一定扑空` —— 把"本文件不可信"写进本文件；
///   - `temp/notes-archive/**`、`工作区 notes/开发记录.md` —— 指向不存在路径的死引用。
///
/// **豁免**：`HISTORY.md`、`patches/*/PATCHES.md` —— 它们是史料，按"只增不改"存档，
/// 考古叙述在那里是**应该有**的。同理，`★ CurSeen 补丁 0NN`
/// 这类标记不是考古（`★` 后紧跟的是补丁号，不含"核查"）。
#[test]
fn test_current_docs_carry_no_archaeology() {
    let root = repo_root();
    // 现行文档（受约束）。史料不在列 —— 见函数文档。
    const CURRENT: &[&str] = &[
        "main/README.md",
        "main/AGENTS.md",
        "main/目录结构说明.md",
        "main/DOCS_MANIFEST.md",
        "main/开发指南.md",
        "main/THIRD-PARTY-NOTICES.md",
        "main/build/build_readme.md",
        "main/patches/README.md",
        "main/docs/DESIGN.md",
        "main/docs/PLATFORM_NOTES.md",
        "main/docs/RELEASE_CHECKLIST.md",
        "main/docs/UI_COMPONENTS.md",
        "main/docs/踩坑记录.md",
        "main/docs/踩坑记录-2.md",
        "main/assets/README.md",
        "main/examples/README.md",
    ];
    // 死引用家族与考古结论。要加新条目先确认它在**现行**文档里确实不该出现。
    const BANNED: &[&str] = &[
        "notes-archive",
        "docs/archive",
        "工作区 `notes/",
        "该目录已不存在",
        "别以为它还在",
    ];

    let mut found: Vec<String> = Vec::new();
    for rel in CURRENT {
        let p = root.join(rel);
        let Ok(text) = std::fs::read_to_string(&p) else {
            panic!("{rel} 读不到——它在 CURRENT 名单里就必须存在");
        };
        for (n, line) in text.lines().enumerate() {
            for pat in BANNED {
                if line.contains(pat) {
                    found.push(format!(
                        "{rel}:{}  含「{pat}」\n      {}",
                        n + 1,
                        line.trim()
                    ));
                }
            }
            if looks_like_archaeology_stamp(line) {
                found.push(format!(
                    "{rel}:{}  似考古日期戳（`★` 后 60 字内出现「核查」）\n      {}",
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "现行文档里出现了考古叙述（历史沿革请写进 HISTORY.md）：\n  {}\n\
         ——现行文档只写「现在是什么」与「该怎么做」，判据见《开发指南.md》§七",
        found.join("\n  ")
    );
}

/// 受控文档必须**保持简洁**：要点进根文档，细节进 `docs/`。
///
/// **为什么有这条**（2026-09-27 定稿）：`HISTORY.md` 一度长到 1357 行 / 61 KB，
/// 其中 `## 2.0.0（开发中）` 一节独占 1340 行——每次落地的实测过程被整段抄了进去，
/// 而同样的内容在别处已经有一份（起因 / 实测数据 / 遗留项各写一遍）。
/// 对照成熟项目 Pecia：它的 `HISTORY.md` 只有 84 行，每版只记「新增 / 修复 / 变更」要点。
///
/// **判据**：超限不是「内容多」，而是**放错了地方**。修法是把过程搬出去（过程件去根
/// `temp/`、坑与教训进 `docs/踩坑记录.md`、设计结论进 `docs/DESIGN.md`），根文档只留
/// 「这一版有什么 / 现在怎么用」。
///
/// **豁免**：`patches/*/PATCHES.md`（补丁登记，编号递增不删节）——它**不在**下面的名单里。
///
/// **踩坑记录是分册的**（2026-10-02 定稿）：它增长最快，压在一份里只能靠「删旧条目腾地方」
/// 续命，而删掉的都是还会再踩的坑。改成**册数不限、每册 ≤ 500 行**：写满就新开一册
/// （`踩坑记录.md` / `踩坑记录-2.md` / …），章节编号跨册唯一，源码注释里的 `《踩坑记录》§N`
/// 照旧按编号查。所以那一档用**发现式**扫描（新增册自动受管，不用回来改名单）。
#[test]
fn test_controlled_docs_stay_concise() {
    let root = repo_root();

    // 根文档与各 README：一眼要能看完。
    const SMALL: &[&str] = &[
        "main/README.md",
        "main/AGENTS.md",
        "main/HISTORY.md",
        "main/DOCS_MANIFEST.md",
        "main/开发指南.md",
        "main/目录结构说明.md",
        "main/THIRD-PARTY-NOTICES.md",
        "main/build/build_readme.md",
        "main/patches/README.md",
        "main/assets/README.md",
        "main/examples/README.md",
    ];
    // 专项技术文档：本来就是手册，允许更长。踩坑记录不在此列（下面按分册扫）。
    const MEDIUM: &[&str] = &[
        "main/docs/DESIGN.md",
        "main/docs/PLATFORM_NOTES.md",
        "main/docs/RELEASE_CHECKLIST.md",
        "main/docs/UI_COMPONENTS.md",
    ];
    const SMALL_MAX: usize = 400;
    const MEDIUM_MAX: usize = 800;
    // 踩坑记录：册数不限，每册 ≤ 500 行（见函数文档）。
    const VOLUME_MAX: usize = 500;

    let mut over: Vec<String> = Vec::new();
    for (list, max) in [(SMALL, SMALL_MAX), (MEDIUM, MEDIUM_MAX)] {
        for rel in list {
            let p = root.join(rel);
            let text = read(&p);
            let n = text.lines().count();
            if n > max {
                over.push(format!("{rel}  {n} 行 > 上限 {max}"));
            }
        }
    }

    // 分册档：发现式扫 `main/docs/踩坑记录*.md`，新增一册即自动受管。
    let docs = root.join("main/docs");
    let mut volumes: Vec<String> = Vec::new();
    for e in std::fs::read_dir(&docs)
        .expect("读 main/docs 目录失败")
        .flatten()
    {
        let p = e.path();
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with("踩坑记录") || !name.ends_with(".md") {
            continue;
        }
        volumes.push(name.clone());
        let n = read(&p).lines().count();
        if n > VOLUME_MAX {
            over.push(format!("main/docs/{name}  {n} 行 > 上限 {VOLUME_MAX}"));
        }
    }
    volumes.sort();
    assert!(
        !volumes.is_empty(),
        "main/docs/ 下没有 `踩坑记录*.md` —— 分册被改名或删掉了，这条门禁就失去了保护意义"
    );

    assert!(
        over.is_empty(),
        "以下受控文档超出篇幅上限——这是「放错了地方」，不是「内容多」：\n  {}\n\
         ——修法：把过程搬出去（过程件去根 temp/、坑与教训进 `docs/踩坑记录*.md`、设计结论进 \
         `docs/DESIGN.md`），根文档只留「这一版有什么 / 现在怎么用」。\n\
         踩坑记录是**分册**的（当前 {volumes:?}）：单册写满 {VOLUME_MAX} 行就新开一册，\
         别删旧条目腾地方。参照 Pecia：它的 HISTORY.md 只有 84 行（只记要点）。判据见《开发指南.md》§五。",
        over.join("\n  ")
    );
}

#[test]
fn test_patch_record_has_a_section_for_every_windui_patch() {
    let root = repo_root();

    // ★ 不要写死路径/版本号。补丁登记表放在 `main/patches/<库名>-<版本>/PATCHES.md`，
    //   目录名带版本号（windui 0.16.1 → 0.19.0 迁移时这里就因为写死而扫错了地方）。
    //   改成发现式：在 `main/patches/` 下找 PATCHES.md，找不到就明确报错。
    let patches_root = root.join("main/patches");
    let mut record_paths: Vec<PathBuf> = Vec::new();
    collect_named(&patches_root, "PATCHES.md", &mut record_paths);
    record_paths.sort();
    assert!(
        !record_paths.is_empty(),
        "在 {patches_root:?} 下没找到任何 PATCHES.md —— \
         补丁登记表缺失（不是「没打补丁」，是「登记表没了」）"
    );
    let mut record = String::new();
    for p in &record_paths {
        record.push_str(&read(p));
        record.push('\n');
    }

    // ★ 不要写死版本号。0.16.1 → 0.19.0 迁移时这个路径没跟着改，于是门禁去扫一个
    //   已经不存在的目录 → `marks` 空 → 断言失败，而失败信息把原因指向"没打补丁"，
    //   完全误导（真实原因是"扫错了地方"）。改成发现式扫描 + 扫不到就明确报错。
    let third_root = root.join(".3rdParty");
    let mut third_dirs: Vec<PathBuf> = std::fs::read_dir(&third_root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("wind-ui-rust-"))
                })
                .map(|p| p.join("src"))
                .collect()
        })
        .unwrap_or_default();
    third_dirs.sort();
    assert!(
        !third_dirs.is_empty(),
        "在 {third_root:?} 下没找到任何 wind-ui-rust-* 源码目录 —— \
         门禁失效了（不是「没打补丁」，是「扫错了地方」）"
    );

    // 收集源码里所有 `CurSeen 补丁 0NN` 标记。
    let mut marks: Vec<String> = Vec::new();
    let mut scanned: Vec<String> = Vec::new();
    for dir in &third_dirs {
        assert!(dir.is_dir(), "缺少 windui 源码目录：{dir:?}");
        scanned.push(format!(
            "{}: {}",
            dir.display(),
            regex_like_scan(dir, &mut marks)
        ));
    }

    assert!(
        !marks.is_empty(),
        "未在 windui 源码中找到任何 'CurSeen 补丁' 标记：{scanned:?}\n\
         ——如果对第三方源码做了修改，必须在每处用 `★ CurSeen 补丁 0NN` 标注（见 AGENTS.md 第六节）"
    );

    let mut unrecorded: Vec<String> = Vec::new();
    for m in &marks {
        // 登记表里应有 `## 补丁 0NN`。
        if !record.contains(&format!("## 补丁 {m}")) {
            unrecorded.push(m.clone());
        }
    }
    assert!(
        unrecorded.is_empty(),
        "以下补丁号在 windui 源码里有标记、却没在 main/patches/**/PATCHES.md 登记章节：{unrecorded:?}\n\
         ——登记表是对依赖升级时唯一的恢复依据，缺失会在重建 .3rdParty 时静默丢失改动"
    );
}

/// 递归收集指定文件名的路径（不引第三方正则库）。
fn collect_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_named(&p, name, out);
        } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            out.push(p);
        }
    }
}

/// 遍历三方源码目录，把所有 `CurSeen 补丁 0NN` 的编号收集起来（不引第三方正则库）。
fn regex_like_scan(dir: &Path, out: &mut Vec<String>) -> String {
    let mut visited = 0usize;
    walk(dir, out, &mut visited);
    out.sort();
    out.dedup();
    format!("已扫描 {visited} 个文件")
}

fn walk(dir: &Path, out: &mut Vec<String>, visited: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out, visited);
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("rs") {
            continue;
        }
        *visited += 1;
        if let Ok(body) = std::fs::read_to_string(&p) {
            collect_ids(&body, out);
        }
    }
}

/// 从文本里抓 `CurSeen 补丁 0NN…` 形式的三位编号。
fn collect_ids(body: &str, out: &mut Vec<String>) {
    const KEY: &str = "CurSeen 补丁 ";
    for (idx, _) in body.match_indices(KEY) {
        let id: String = body[idx + KEY.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if id.len() == 3 {
            out.push(id);
        }
    }
}

/// 面板源码里用到的**展开态 id** 必须都在 `CARD_IDS` 里。
///
/// # 为什么值得一条门禁
///
/// `PanelModel::expand(id)` 对未知 id 会**静默**回落到一个兜底信号（不报错、不 panic，
/// 见 `ui::model` 里 `no_expand` 的注释）——这是刻意的（老配置里可能有已删除的键），
/// 但代价是把 `record.video.audio` 写成 `record.video.sound` 的后果是"那张子卡永远展不开
/// （或永远展得开）"，而**没有任何提示**：用户只会说"面板又抽风了"，开发者只会去怀疑渲染层。
///
/// # 扫描的局限（已用 `checked` 下限兜住）
///
/// 只认**同一行**的 `expand("literal")` 写法。用 `format!("ripple.{key}")` 拼出来的那批
/// 在源码里带 `{}`，静态扫不出真实值 —— 它们由 `config.rs` 的
/// `card_ids_follow_the_naming_rule` 从命名规则侧兜住，这里显式跳过。
#[test]
fn test_panel_expand_ids_exist_in_card_ids() {
    use curseen_ui::core::config::CARD_IDS;

    let root = repo_root();
    let dir = root.join("main/src/ui/pages");
    let mut checked = 0usize;
    let mut unknown: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .expect("读 ui/pages 目录失败")
        .flatten()
    {
        let p = entry.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rs") {
            continue;
        }
        let body = read(&p);
        let file = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for id in scan_expand_ids(&body) {
            if id.contains('{') {
                continue; // `format!` 拼出来的，交给 CARD_IDS 命名规则那条单测
            }
            checked += 1;
            if !CARD_IDS.contains(&id.as_str()) {
                unknown.push(format!("{file}: {id}"));
            }
        }
    }
    // 下限只用来兜住"扫描逻辑自己坏了"（正则/标记串被改坏时上面会一个都扫不到，
    // 于是断言恒真、门禁变成摆设）。当前实测 21 条，取 15 留出删页面的余地。
    assert!(
        checked >= 15,
        "静态扫到的展开 id 太少（{checked} 条）——扫描逻辑可能已失效，门禁形同虚设"
    );
    assert!(
        unknown.is_empty(),
        "以下展开 id 不在 CARD_IDS 里：{unknown:?}\n\
         —— `PanelModel::expand` 对未知 id 不会报错，只会让那张卡永远展不开/永远展得开。\n\
         请二选一：把 id 改成 CARD_IDS 里的那个，或把新 id 加进 CARD_IDS。"
    );
}

/// 仓库卫生：`main/` 是要上传 git 的目录，布局对齐 pecia 两分法（2026-09-25）。
///
/// # 为什么值得一条门禁
///
/// 过程性文档混进源码树有两个坏处：半年后没人维护，却会被后来者当成"现行约定"读；
/// 而且它会让仓库被几百 KB 的一次性文字淹没。本次重构就是从这个坑里爬出来的——
/// `main/docs/archive/` 里堆着 8 份 2026-09 的方案与评估，`main/开发指引.md` 长到
/// 249 KB（后半全是逐次开发的落地流水）。
///
/// 两分法只有两类，没有第三类（原"项目根 notes/"灰色目录已于 2026-09-25 撤销）：
/// - **有长期价值 → 入库**：现行文档（设计 / 方案 / 说明）放 `main/docs/`，
///   且必须在 DOCS_MANIFEST.md 登记；
/// - **一次性 / 仅追溯 → 仓库外可丢区**：根 `temp/`（含 `temp/notes-archive/`）。
///
/// 本条钉三件可判定的事实：
/// 1. `main/` 根目录的 `.md` 只允许白名单里的那些（防止随手往仓库根丢文档）；
/// 2. `main/docs/` 下不许出现归档目录；
/// 3. `main/docs/` 下每个 `.md` 必须登记在 DOCS_MANIFEST.md（防止未受控文档溜进仓库）。
#[test]
fn test_repo_holds_no_process_documents() {
    let root = repo_root();
    let main_dir = root.join("main");

    // (1) 根目录白名单。新增受控文档时请同时更新本表与 DOCS_MANIFEST.md。
    //   UI_DESIGN.md 已于 2026-09-25 移除（09-13 的 plan 文档，与现行实现脱节；
    //   视觉结论以 docs/DESIGN.md 与 ui/ 源码为准）。
    let allowed = [
        "README.md",
        "AGENTS.md",
        "DOCS_MANIFEST.md",
        "目录结构说明.md",
        "开发指南.md",
        "HISTORY.md",
        "THIRD-PARTY-NOTICES.md",
    ];
    let mut stray: Vec<String> = Vec::new();
    for e in std::fs::read_dir(&main_dir)
        .expect("读 main 目录失败")
        .flatten()
    {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !allowed.contains(&name.as_str()) {
            stray.push(name);
        }
    }
    assert!(
        stray.is_empty(),
        "main/ 根目录出现了白名单外的文档：{stray:?}\n\
         ——仓库根只放长期受控文档。一次性草稿 / 实测转储请放根 temp/（不入库），\n\
         长期文档请放 main/docs/ 并在 DOCS_MANIFEST.md 登记"
    );

    // (2) docs/ 下不许再有归档目录——历史件去根 temp/notes-archive/（可丢，仅追溯）。
    let docs = main_dir.join("docs");
    assert!(
        !docs.join("archive").exists(),
        "main/docs/archive/ 不应存在 —— 归档类文档请移到项目根 temp/notes-archive/"
    );

    // (3) docs/ 下每个 .md 必须登记在 DOCS_MANIFEST.md——单一来源防"随手一丢"。
    //   自证：docs 下至少 3 份现行文档（DESIGN / PLATFORM_NOTES / RELEASE_CHECKLIST；
    //   踩坑记录也在 docs/），扫不到这个量说明目录结构变了，
    //   本断言就失去了保护意义，必须跟着改。
    let manifest = read(&main_dir.join("DOCS_MANIFEST.md"));
    let mut unregistered: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for e in std::fs::read_dir(&docs)
        .expect("读 main/docs 目录失败")
        .flatten()
    {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        seen += 1;
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !manifest.contains(&name) {
            unregistered.push(format!("docs/{name}"));
        }
    }
    assert!(
        seen >= 3,
        "main/docs/ 下只扫到 {seen} 份 .md（预期 >= 3）——目录结构变了，\
         请同步修订本门禁的基线数量"
    );
    assert!(
        unregistered.is_empty(),
        "main/docs/ 下出现了未登记的文档：{unregistered:?}\n\
         ——要么在 DOCS_MANIFEST.md 登记（入库受控），要么移出 main/（一次性件去根 temp/）"
    );
}

/// 扫出源码里 `expand("…")` 的字符串字面量（不引第三方正则库）。
fn scan_expand_ids(src: &str) -> Vec<String> {
    const MARK: &str = "expand(\"";
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < src.len() {
        let Some(pos) = src[i..].find(MARK) else {
            break;
        };
        let start = i + pos + MARK.len();
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'"' {
            j += 1;
        }
        if j >= bytes.len() {
            break; // 没找到收尾引号：源码不完整，别硬编
        }
        if j > start {
            out.push(src[start..j].to_string());
        }
        i = j + 1; // 引号是 ASCII，+1 一定落在字符边界上
    }
    out
}

// —— m23：`API.md` 与《Lua 脚本手册》第二部分**同一段表格的两副面孔**，等值由机器读 ——
//
// 两份文件描述的是同一件事（同一批接口、同一批元数据）：`assets/lua/API.md` 是查签名的参考，
// `assets/lua/Lua脚本手册.html` 是脚本作者实际会点开的交付件。它们过去**只靠人记得同步**，
// 于是 m10–m19 陆续开出去的 `proc` / `read_file` / `set_image` / `clip_*` / `crop` …
// **在手册里一个都不存在**（本轮 grep 实测：手册的 `ext:` 只有 6 个名字）。
// 判据口径：比对**名字 + 签名**（两副面孔里唯一真正重复的事实），**刻意不比"说明"栏**——
// 那一栏两份各写各的、本来就该不一样，逼它等值只会得到两份互相抄的废话。

/// 把 markdown 的一行表格按**未转义**的 `|` 切成单元格（单元格里的 `\|` 是签名的一部分）。
fn md_row_cells(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            cur.push(ch);
            esc = false;
            continue;
        }
        match ch {
            '\\' => {
                cur.push(ch);
                esc = true;
            }
            '|' => {
                cells.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    cells.push(cur);
    cells
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

/// 去反引号 / 去 HTML 标签 / 解实体 / 并空白 —— 两副面孔归一到同一形态。
fn norm_api_cell(raw: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        // 只把**真标签**（`<` 后紧跟字母或 `/`）当标记吃掉。上一版见 `<` 就进标签态，
        // 于是签名里的 `->` 被当成标签吞掉半个字符，两副面孔比出来"不一样"——红的是量具。
        let is_tag =
            c == '<' && i + 1 < cs.len() && (cs[i + 1].is_ascii_alphabetic() || cs[i + 1] == '/');
        if is_tag {
            while i < cs.len() && cs[i] != '>' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c != '`' {
            out.push(c);
        }
        i += 1;
    }
    // markdown 表格里的 `' + BS + '|` 是转义（不转义会把表格切多一列），渲染出来是 `|`；
    // 两份比对前都还原成 `|`，否则手册与 API.md 永远"差一个反斜杠"。
    let un = out
        .replace("\\|", "|")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    un.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 从 `API.md` 里取"表头等于 `header` 的那张表"的 (第 1 列, 第 3 列) 清单，按出现顺序。
fn md_table_pairs(md: &str, header: &[&str]) -> Vec<Vec<(String, String)>> {
    let mut tables = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let cells = md_row_cells(lines[i]);
        let is_head = cells.len() >= header.len()
            && cells[..header.len()]
                .iter()
                .zip(header)
                .all(|(c, h)| *c == *h);
        if !is_head {
            i += 1;
            continue;
        }
        let mut rows = Vec::new();
        i += 2; // 跳过分隔行
        while i < lines.len() && lines[i].trim_start().starts_with('|') {
            let c = md_row_cells(lines[i]);
            if c.len() >= 3 {
                rows.push((norm_api_cell(&c[0]), norm_api_cell(&c[2])));
            }
            i += 1;
        }
        tables.push(rows);
    }
    tables
}

/// 从手册 HTML 里取"表头等于 `header` 的那张表"的同样清单。
fn html_table_pairs(html: &str, header: &[&str]) -> Vec<Vec<(String, String)>> {
    let mut tables = Vec::new();
    let mut rest = html;
    while let Some(tstart) = rest.find("<table>") {
        let Some(tend) = rest[tstart..].find("</table>") else {
            break;
        };
        let block = &rest[tstart..tstart + tend];
        let ths: Vec<String> = block
            .split("<th>")
            .skip(1)
            .map(|s| norm_api_cell(s.split("</th>").next().unwrap_or(s)))
            .collect();
        let matched = header.len() == ths.len() && ths.iter().zip(header).all(|(c, h)| *c == *h);
        if matched {
            let mut rows = Vec::new();
            for tr in block.split("<tr>").skip(1) {
                let tds: Vec<String> = tr
                    .split("<td>")
                    .skip(1)
                    .map(|s| norm_api_cell(s.split("</td>").next().unwrap_or(s)))
                    .collect();
                if tds.len() >= 3 {
                    rows.push((tds[0].clone(), tds[2].clone()));
                }
            }
            tables.push(rows);
        }
        rest = &rest[tstart + tend..];
    }
    tables
}

/// 两副面孔逐张比对，返回"第 n 张表：手册少了 X、多了 Y、签名不一致 Z"这样的人话。
fn diff_api_tables(md: &str, html: &str, header: &[&str], what: &str) -> Vec<String> {
    let a = md_table_pairs(md, header);
    let b = html_table_pairs(html, header);
    if a.len() != b.len() {
        return vec![format!(
            "{what}：`API.md` 里有 {} 张这样的表，手册里只有 {} 张——两份的形状已经不对了",
            a.len(),
            b.len()
        )];
    }
    let mut out = Vec::new();
    for (idx, (ma, mb)) in a.iter().zip(b.iter()).enumerate() {
        for (name, sig) in ma {
            match mb.iter().find(|(n, _)| n == name) {
                None => out.push(format!("{what} 表#{idx}：手册**缺**「{name}」")),
                Some((_, hs)) if hs != sig => out.push(format!(
                    "{what} 表#{idx}：「{name}」签名两份不一致\n  API.md: {sig}\n  手册: {hs}"
                )),
                Some(_) => {}
            }
        }
        for (name, _) in mb {
            if !ma.iter().any(|(n, _)| n == name) {
                out.push(format!(
                    "{what} 表#{idx}：手册里多出「{name}」（`API.md` 没有）"
                ));
            }
        }
    }
    out
}

/// ★ **手册必须与 `API.md` 的接口表逐签名等值**（m23）。这条读的是文本真源、不参与渲染，
/// 所以没有"看着像对"的空间：加一个接口而忘了同步交付件，CI 当场点名是哪个名字。
#[test]
fn test_lua_manual_matches_the_api_reference_tables() {
    let root = repo_root();
    let md = read(&root.join("main/assets/lua/API.md"));
    let html = read(&root.join("main/assets/lua/Lua脚本手册.html"));
    let mut offences = Vec::new();
    offences.extend(diff_api_tables(
        &md,
        &html,
        &["成员", "形式", "签名 / 返回", "说明"],
        "ext / ExtImage 成员表",
    ));
    offences.extend(diff_api_tables(
        &md,
        &html,
        &["全局名", "类型", "读取方", "说明"],
        "脚本可声明的顶层全局表",
    ));
    assert!(
        offences.is_empty(),
        "《Lua 脚本手册》与 `API.md` 已经飘了（两份描述同一事实，只有这一格在机器读它们）：\n{}",
        offences.join("\n")
    );
}
