//! 在线脚本包的后台动作（m36）：拉清单、下载 zip、安装/卸载，进展经 `UiMsg::Store` 回 UI 线程。
//!
//! 与 `update::task` 同形：一个 `BusyGuard` 保证同一时刻只有一个动作；线程里跑阻塞网络与磁盘，
//! UI 线程只收事件。
//!
//! # 数据从哪来 / 存到哪（m43：不再有全局记录目录）
//!
//! - 清单：官方双镜像（选中镜像优先，失败再试另一个），成功一次就写进**随包的**
//!   `<exe>/assets/docs/scripts.json`（RuPix 同款：docs 里的 JSON，联网更新替换它）；
//!   两个都拉不到时回退这份文件并注明"离线"。
//! - 包 zip：下到 `<exe>/temp/script_store/<id>.zip`，装完即删（不留第二份）。
//! - 安装记录：写在**包自己目录**里的 `.curseen-pack.json`（由 `scripting::store` 读写）。
//!
//! ★ 装完/卸完都**主动重发一次行表**（`CatalogReady`）：安装徽标要立刻跟上，不能等下一次刷新。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windui::prelude::Sender;

use crate::core::events::{StoreAction, StoreEvent, StorePackRow, UiMsg};
use crate::scripting::store::{self, CatalogPack};

const CATALOG_MAX_BYTES: u64 = 4 << 20;

/// 随包清单：`<exe>/assets/docs/scripts.json`（联网刷新会替换它）。
pub fn catalog_path() -> PathBuf {
    crate::core::paths::asset_dir("docs").join("scripts.json")
}

/// 脚本根（`assets/lua`）：安装/卸载/读已装记录都以它为根。
fn assets_root() -> PathBuf {
    crate::core::paths::lua_ext_dir()
}

/// 最近一次成功解析的清单（进程内）。安装动作按 id 从这里取下载地址。
fn catalog_cache() -> &'static Mutex<Vec<CatalogPack>> {
    static C: OnceLock<Mutex<Vec<CatalogPack>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// 同一时刻只跑一个动作（连点不会叠出两条线程写同一个 zip）。
static BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl BusyGuard {
    fn take() -> Option<Self> {
        BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// 两条地址的尝试顺序：**选中的镜像在前**，空串（该来源未配置）跳过。
///
/// `source` 夹到 0..=1——配置里手写成 9 时不许越界。
pub(crate) fn ordered_urls(source: usize, urls: &[String; 2]) -> Vec<(usize, &str)> {
    let source = source.min(1);
    let mut out = Vec::new();
    for src in [source, 1 - source] {
        let url = urls[src].trim();
        if !url.is_empty() {
            out.push((src, url));
        }
    }
    out
}

/// 拉清单（阻塞）。先试 `source`，失败再试另一个镜像；都失败才回退随包清单。
///
/// 返回 `(packs, note)`；`note` 非空表示"有话说"（用了另一镜像 / 跳过的条目 / 离线）。
pub fn refresh_blocking(
    source: usize,
    urls: &[String; 2],
) -> Result<(Vec<CatalogPack>, String), String> {
    let order = ordered_urls(source, urls);
    if order.is_empty() {
        return Err(
            "清单地址为空（config.json 的 store.gitee_url / store.github_url）".to_string(),
        );
    }
    let mut errors = Vec::new();
    for (n, &(src, url)) in order.iter().enumerate() {
        match crate::platform::http::get_text(url, CATALOG_MAX_BYTES) {
            Ok(text) => match store::parse_catalog(&text) {
                Ok((packs, skipped)) => {
                    *catalog_cache().lock().unwrap_or_else(|e| e.into_inner()) = packs.clone();
                    if let Some(dir) = catalog_path().parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(catalog_path(), text.as_bytes());
                    let mut note = String::new();
                    if n > 0 {
                        note.push_str(&format!("已从 {} 镜像获取", store::SOURCE_LABELS[src]));
                    }
                    if !skipped.is_empty() {
                        if !note.is_empty() {
                            note.push_str(" · ");
                        }
                        note.push_str(&format!("跳过 {} 项信息不全", skipped.len()));
                    }
                    return Ok((packs, note));
                }
                Err(e) => errors.push(format!("{url}：{e}")),
            },
            Err(e) => errors.push(format!("{url}：{e}")),
        }
    }
    if let Ok(text) = std::fs::read_to_string(catalog_path()) {
        if let Ok((packs, _)) = store::parse_catalog(&text) {
            *catalog_cache().lock().unwrap_or_else(|e| e.into_inner()) = packs.clone();
            return Ok((packs, "离线：显示上次缓存的清单".to_string()));
        }
    }
    Err(format!("拉取清单失败：{}", errors.join("；")))
}

/// 清单 × 本机记录 → 行表（`too_new` = 包的 `min_host` 高于当前宿主）。
///
/// ★ **并集，不是交集**：本机装着、但当前清单里没有的包（离线/作者下架）也要成行——
///   否则重开面板后没联网刷新就看不到它、也就卸载不了。
pub fn rows_of(
    catalog: &[CatalogPack],
    installed: &[store::InstalledPack],
    host_version: &str,
) -> Vec<StorePackRow> {
    let mut rows: Vec<StorePackRow> = catalog
        .iter()
        .map(|p| {
            let rec = installed.iter().find(|i| i.manifest.id == p.id);
            StorePackRow {
                id: p.id.clone(),
                name: p.name.clone(),
                version: p.version.clone(),
                description: p.description.clone(),
                size: p.size,
                installed_version: rec.map(|i| i.manifest.version.clone()),
                too_new: p
                    .min_host
                    .as_deref()
                    .is_some_and(|m| store::version_newer(m, host_version))
                    || p.api > store::LUA_API,
                files: rec.map(|i| i.manifest.files.len() as u64).unwrap_or(0),
                source_mask: p.source_mask(),
                author: p.author.clone(),
                license: p.license.clone(),
                released: p.released.clone(),
                api: p.api,
                unpacked_size: p.unpacked_size,
                file_count: p.file_count,
            }
        })
        .collect();
    for i in installed {
        if catalog.iter().any(|p| p.id == i.manifest.id) {
            continue;
        }
        rows.push(StorePackRow {
            id: i.manifest.id.clone(),
            name: i.manifest.name.clone(),
            version: i.manifest.version.clone(),
            description: String::new(),
            size: 0,
            installed_version: Some(i.manifest.version.clone()),
            too_new: false,
            files: i.manifest.files.len() as u64,
            // 清单里没有它 ⇒ 没有在线地址（行尾按钮只能是灰的；详情窗里可以卸载）。
            source_mask: 0,
            author: String::new(),
            license: String::new(),
            released: String::new(),
            api: 0,
            unpacked_size: 0,
            file_count: i.manifest.files.len() as u64,
        });
    }
    rows
}

/// 当前"清单 × 本机记录"的行表（清单来自进程内缓存，不联网）。
fn current_rows() -> Vec<StorePackRow> {
    let packs = catalog_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    rows_of(
        &packs,
        &store::installed_packs(&assets_root()),
        env!("CARGO_PKG_VERSION"),
    )
}

/// 开机即用的行表：**读随包清单 + 已装记录，不联网**。
///
/// 面板打开时先有这张表（已装的包离线也能看到、能卸载），点「刷新」才联网更新。
/// 清单解析成功时同时回填进程内清单——否则重启后"界面有下载按钮、点下去却报清单里没有"。
pub fn cached_rows() -> Vec<StorePackRow> {
    let packs = std::fs::read_to_string(catalog_path())
        .ok()
        .and_then(|text| store::parse_catalog(&text).ok().map(|(packs, _)| packs))
        .unwrap_or_default();
    if !packs.is_empty() {
        *catalog_cache().lock().unwrap_or_else(|e| e.into_inner()) = packs.clone();
    }
    rows_of(
        &packs,
        &store::installed_packs(&assets_root()),
        env!("CARGO_PKG_VERSION"),
    )
}

/// 下载并安装一个包（阻塞）。`progress(已收, 总长)` 由下载层回调。
///
/// 先试选中镜像的地址，失败再试另一个（清单里两个地址都有的包才保证这条路）。
pub fn install_blocking(
    pack: &CatalogPack,
    source: usize,
    mut progress: impl FnMut(u64, u64),
) -> Result<store::InstallReport, String> {
    let source = source.min(1);
    let dir = crate::core::paths::app_dir()
        .join("temp")
        .join("script_store");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建下载目录失败：{e}"))?;
    let zip = dir.join(format!("{}.zip", pack.id));
    let headers = [(
        "User-Agent".to_string(),
        crate::platform::http::DEFAULT_UA.to_string(),
    )];
    let mut last_err = None;
    let mut ok = false;
    for src in [source, 1 - source] {
        let Some(url) = pack.url_for(src) else {
            continue;
        };
        match crate::platform::http::download_to_file(
            url,
            &zip,
            &headers,
            crate::platform::http::SCRIPT_DOWNLOAD_MAX,
            &mut progress,
        ) {
            Ok(_) => {
                ok = true;
                break;
            }
            Err(e) => last_err = Some(format!("{url}：{e}")),
        }
    }
    if !ok {
        return Err(format!(
            "下载失败：{}",
            last_err.unwrap_or_else(|| "清单里没有可用的下载地址".to_string())
        ));
    }
    let r = store::install_zip(&zip, &assets_root());
    let _ = std::fs::remove_file(&zip);
    r
}

/// 发起一个动作：立即返回，事件稍后经 `tx` 回 UI 线程。
pub fn start(
    tx: &Option<Sender<UiMsg>>,
    action: StoreAction,
    urls: [String; 2],
) -> Result<(), String> {
    let tx = tx.clone().ok_or_else(|| "UI 通道尚未就绪".to_string())?;
    let Some(busy) = BusyGuard::take() else {
        return Err("上一个脚本库动作还在进行".to_string());
    };
    std::thread::Builder::new()
        .name("fx-script-store".into())
        .spawn(move || {
            let _busy = busy;
            let send = |ev: StoreEvent| {
                let _ = tx.send(UiMsg::Store(ev));
            };
            match action {
                StoreAction::RefreshCatalog { source } => {
                    send(StoreEvent::CatalogStarted);
                    match refresh_blocking(source, &urls) {
                        Ok((packs, note)) => send(StoreEvent::CatalogReady {
                            rows: rows_of(
                                &packs,
                                &store::installed_packs(&assets_root()),
                                env!("CARGO_PKG_VERSION"),
                            ),
                            note,
                        }),
                        Err(error) => send(StoreEvent::CatalogFailed { error }),
                    }
                }
                StoreAction::Install { id, source } => {
                    let pack = catalog_cache()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .find(|p| p.id == id)
                        .cloned();
                    let Some(pack) = pack else {
                        send(StoreEvent::InstallFailed {
                            id,
                            error: "清单里没有这一项（先点「刷新」）".to_string(),
                        });
                        return;
                    };
                    let progress_id = pack.id.clone();
                    let tx2 = tx.clone();
                    let mut last_pct = i32::MIN;
                    let r = install_blocking(&pack, source, move |received, total| {
                        let pct = received
                            .saturating_mul(100)
                            .checked_div(total)
                            .map_or(-1, |p| p.min(100) as i32);
                        if pct != last_pct {
                            last_pct = pct;
                            let _ = tx2.send(UiMsg::Store(StoreEvent::PackProgress {
                                id: progress_id.clone(),
                                received,
                                total,
                            }));
                        }
                    });
                    match r {
                        Ok(rep) => {
                            // ★ 顺序：**先行表、后结果**。反过来的话「已安装…」这句会被
                            //   紧随其后的摘要盖掉，用户看不到这次干了什么。
                            send(StoreEvent::CatalogReady {
                                rows: current_rows(),
                                note: String::new(),
                            });
                            send(StoreEvent::Installed {
                                id: rep.id,
                                name: rep.name,
                                version: rep.version,
                                added: rep.added as u64,
                                updated: rep.updated as u64,
                                removed: rep.removed as u64,
                            });
                        }
                        Err(error) => send(StoreEvent::InstallFailed { id, error }),
                    }
                }
                StoreAction::Uninstall(id) => {
                    let r = store::uninstall(&id, &assets_root());
                    match r {
                        Ok(files) => {
                            send(StoreEvent::CatalogReady {
                                rows: current_rows(),
                                note: String::new(),
                            });
                            send(StoreEvent::Uninstalled {
                                id,
                                files: files as u64,
                            });
                        }
                        Err(error) => send(StoreEvent::UninstallFailed { id, error }),
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|e| format!("起脚本库线程失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(
        id: &str,
        version: &str,
        min_host: Option<&str>,
        urls: [Option<&str>; 2],
    ) -> CatalogPack {
        CatalogPack {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            description: String::new(),
            size: 1,
            urls: urls.map(|u| u.map(str::to_string)),
            min_host: min_host.map(str::to_string),
            author: "CurSeen".to_string(),
            license: "AGPL-3.0".to_string(),
            released: "2026-10-28".to_string(),
            api: 1,
            unpacked_size: 2048,
            file_count: 2,
        }
    }

    /// 行表合并：装没装、要不要更新、`min_host` 拦不拦、**离线时已装的还在不在**、
    /// **选中镜像有没有地址**，都由这一格说清（纯函数，无网络）。
    #[test]
    fn rows_merge_catalog_with_installed_records() {
        let catalog = vec![
            pack(
                "p.new",
                "1.2.0",
                None,
                [Some("https://g/1.zip"), Some("https://h/1.zip")],
            ),
            pack(
                "p.same",
                "1.0.0",
                None,
                [Some("https://g/2.zip"), Some("https://h/2.zip")],
            ),
            pack(
                "p.future",
                "9.0.0",
                Some("99.0"),
                [Some("https://g/3.zip"), None],
            ),
        ];
        let installed = vec![
            store::InstalledPack {
                manifest: store::PackManifest {
                    id: "p.same".to_string(),
                    name: "p.same".to_string(),
                    version: "1.0.0".to_string(),
                    files: vec!["scripts/a.lua".to_string()],
                },
                record: None,
            },
            // 清单里没有它（离线缓存过期 / 作者下架）：仍要成行，否则卸载不了。
            store::InstalledPack {
                manifest: store::PackManifest {
                    id: "p.local".to_string(),
                    name: "本机包".to_string(),
                    version: "2.0.0".to_string(),
                    files: vec!["scripts/x.lua".to_string(), "scripts/y.lua".to_string()],
                },
                record: None,
            },
        ];
        let rows = rows_of(&catalog, &installed, "2.0.5");
        assert_eq!(rows.len(), 4, "三条清单 + 一条只在本书里");
        assert_eq!(rows[0].installed_version, None, "没装的要如实说没装");
        assert_eq!(rows[0].files, 0, "没装就不知道文件数");
        assert_eq!(rows[0].source_mask, 0b11, "双镜像都有地址");
        assert_eq!(rows[1].installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(rows[1].files, 1);
        assert!(rows[2].too_new, "min_host 99.0 高于 2.0.5 ⇒ 置灰");
        assert_eq!(
            rows[2].source_mask, 0b01,
            "只发了 Gitee ⇒ 选 GitHub 时按钮要灰"
        );
        assert!(!rows[0].too_new);
        assert_eq!(rows[3].id, "p.local");
        assert_eq!(rows[3].installed_version.as_deref(), Some("2.0.0"));
        assert_eq!(rows[3].files, 2);
        assert_eq!(rows[3].source_mask, 0, "清单里没有它 ⇒ 没有在线地址");
        // 清单整个缺失（没缓存/解析失败）时，已装的照样列出来。
        let only_local = rows_of(&[], &installed, "2.0.5");
        assert_eq!(only_local.len(), 2);
        assert!(only_local.iter().all(|r| r.installed_version.is_some()));
    }

    /// 地址顺序（m40）：选中的镜像在前，空串跳过，越界夹到 1。
    #[test]
    fn ordered_urls_try_the_selected_mirror_first_and_skip_blanks() {
        let both = [
            "https://g/scripts.json".to_string(),
            "https://h/scripts.json".to_string(),
        ];
        assert_eq!(
            ordered_urls(1, &both),
            vec![(1, "https://h/scripts.json"), (0, "https://g/scripts.json")]
        );
        assert_eq!(ordered_urls(9, &both)[0].0, 1, "越界夹到 GitHub");
        let gitee_only = ["https://g/scripts.json".to_string(), String::new()];
        assert_eq!(
            ordered_urls(1, &gitee_only),
            vec![(0, "https://g/scripts.json")],
            "GitHub 没配 ⇒ 只试 Gitee"
        );
        let none = [String::new(), "   ".to_string()];
        assert!(ordered_urls(0, &none).is_empty(), "两条都空 ⇒ 没有可试的");
    }
}
