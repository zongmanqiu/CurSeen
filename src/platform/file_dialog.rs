//! 原生「打开文件 / 另存为 / 选择文件夹」对话框（仅 Windows）。
//!
//! 四个入口，落在两组底层 API 上：
//!
//! | 入口 | API | 调用方 |
//! |---|---|---|
//! | [`open_image`] / [`open_gif`] | `GetOpenFileNameW` | 悬浮标「浏览…」/ 裁剪窗「打开…」 |
//! | [`save_png`] / [`save_gif`] / [`save_mp4`] / [`save_text`] | `GetSaveFileNameW` | 贴图「另存为」/ 裁剪窗「保存」/ 视频裁剪 / AI 对话「保存回复…」 |
//! | [`open_folder`] | `IFileOpenDialog` + `FOS_PICKFOLDERS` | （当前无调用方，留作平台能力） |
//!
//! 目录选择特意**不用**老式的 `SHBrowseForFolder`：它没有地址栏、不能粘贴路径，选目录体验
//! 很差，而 Vista 起的 `IFileDialog` 是同一套 COM 接口，只是多设一个选项位。
//!
//! # 两个参数决定"用起来顺不顺"
//!
//! * `start_dir` —— **初始目录**（`lpstrInitialDir`）。不给就交给系统记住上次的位置，
//!   而系统记得的位置对"刚录完的 GIF 在哪"一无所知。
//! * `owner` —— **属主窗口**（`hwndOwner`）。给了属主，对话框就**模态于它**：属主被禁用、
//!   对话框永远在它之上、Esc 只作用于对话框。裁剪窗尤其需要（见下）。
//!
//! ⚠️ 只给 `start_dir` / `owner` 是**提示**，系统可以不理会（比如用户上次手动改过目录）；
//! 所以任何依赖这两者的逻辑都不能当成保证。
//!
//! # 为什么必须经 `ctx.defer_blocking` 延后
//!
//! 文件对话框自带消息泵、会阻塞到松开为止；从窗口过程里直接调就是重入（那条路上再取一次
//! 窗口状态就是 `&mut` 别名，平台的铁律 6）。所以四个入口的返回类型都是"一次性的"，调用方
//! 拿到路径后再做自己的事。这条延后是 windui 的约定：闭包在**事件分发完全返回之后**才跑。
//!
//! ★ 但"延后"不等于"不会重入"：延后闭包是在**窗口过程内部**执行的（windui 的
//!   `apply_dialog_request`），而模态对话框会在它的消息泵里继续给我们的窗口分发消息。
//!   于是"对话框开着时再点一次同一个按钮"会**套出第二层对话框**——实测裁剪窗上连点三次
//!   「打开…」得到了三层看不见的对话框（`build/log` 里三条连续"用户在「打开」里取消"）。
//!   两道防线缺一不可：**属主窗口**（点不到）+ 调用方自己的**重入守卫**。

use std::path::{Path, PathBuf};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

/// 文件路径缓冲区容量（含终止 NUL）。用户可能从超长网络路径选文件，留足 8K 单元余量，
/// 避免截断。
const BUF_LEN: usize = 8192;

/// 文件类型过滤器（PNG/GIF，外加「所有文件」兜底）。格式为 `描述\0通配符\0` 成对、末尾双 NUL。
const FILTER: &str = "图片 (*.png;*.gif)\0*.png;*.gif\0所有文件 (*.*)\0*.*\0\0";

/// PNG 另存为的过滤器（贴图「另存为」用）。
const FILTER_PNG: &str = "PNG 图片 (*.png)\0*.png\0所有文件 (*.*)\0*.*\0\0";

/// GIF 的过滤器（裁剪窗「打开…」/「保存」用）。
///
/// ★ 单独开一份而**不复用** [`FILTER`]：那张是给「悬浮标图片」用的，PNG/GIF 通吃。
///   裁剪窗只吃 GIF，过滤器收窄到 `*.gif` 能省掉一次"选错了才发现打不开"。
const FILTER_GIF: &str = "GIF 动图 (*.gif)\0*.gif\0所有文件 (*.*)\0*.*\0\0";

/// 裁剪窗「打开…」的对话框标题。
const TITLE_OPEN_GIF: &str = "选择要裁剪的 GIF\0";

/// 裁剪窗「保存」的对话框标题。
const TITLE_SAVE_GIF: &str = "裁剪后另存为\0";

/// 对话框标题（带终止 NUL 的 UTF-16）。
const TITLE: &str = "选择悬浮标图片\0";

/// 贴图另存为对话框标题。
const TITLE_SAVE_PNG: &str = "贴图另存为\0";

/// 默认扩展名（用户没写后缀时补上）。
const DEF_EXT_PNG: &str = "png\0";

/// GIF 的默认扩展名。
const DEF_EXT_GIF: &str = "gif\0";

/// 把对话框回填的 UTF-16 缓冲区读成路径（截到终止 NUL；空则 `None`）。
fn buf_to_path(buf: &[u16]) -> Option<PathBuf> {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// 调用方给的裸句柄 → `HWND`；`None` / 0 都当"没有属主"。
///
/// `HWND::default()` 就是空句柄：`GetOpenFileNameW` 收到空属主时对话框**不属于任何人**，
/// 于是它既不会禁用我们的窗口、也不会被抬到我们窗口之上 —— 裁剪窗那三个叠起来的对话框
/// 就是这么来的。
fn owner_hwnd(owner: Option<isize>) -> HWND {
    match owner {
        Some(h) if h != 0 => HWND(h as *mut _),
        _ => HWND::default(),
    }
}

/// 初始目录寄存器：返回的 `Vec` 必须在 `OPENFILENAMEW` 活着期间一直活着（`lpstrInitialDir`
/// 只是借了个指针）。
///
/// 空 `Vec` = 不给初始目录。**不能**改成"空串指针"：那会被当成"目录名是空字符串"。
fn dir_wide(start_dir: Option<&Path>) -> Vec<u16> {
    start_dir
        .map(|p| p.to_string_lossy().encode_utf16().chain([0u16]).collect())
        .unwrap_or_default()
}

/// `lpstrInitialDir` 要的那个指针：没给目录时是 `null`。
fn dir_ptr(dir: &[u16]) -> PCWSTR {
    if dir.is_empty() {
        PCWSTR::null()
    } else {
        PCWSTR::from_raw(dir.as_ptr())
    }
}

/// 对话框返回 FALSE 时把系统的扩展错误码写进日志。
///
/// ★ 区分两种 FALSE：**用户取消**（错误码 0，正常路径，静默）与**对话框根本没建起来**
///   （非 0，比如 `CDERR_*` / `FNERR_*`）。不做这一步，两者在日志里长得一模一样 ——
///   而"点了按钮毫无反应"到底是哪个，正是最难猜的那类问题。
fn log_ext_error(what: &str) {
    let code = unsafe { CommDlgExtendedError() };
    if code.0 != 0 {
        log::warn!(
            "{what}：对话框创建失败，CommDlgExtendedError = 0x{:04X}",
            code.0
        );
    }
}

/// 原生对话框用完之后的收尾：把它带来的那一批页从工作集里摘出去。
///
/// # 为什么需要它（2026-10-01 实测，见 `examples/mem_attrib.rs`）
///
/// 一次「另存为」/「打开」对话框会让进程凭空多出 **90 个 DLL**——`comdlg32` 之后跟着
/// 整个 shell 全家桶：`ExplorerFrame`、`propsys`、`windows.storage`、`d2d1`/`dwrite`、
/// `textinputframework`、`uiautomationcore`，外加本机装的所有命名空间 / 缩略图 / 右键
/// 菜单 shell 扩展。代价是 **+9.7 MB 私有提交**。
///
/// 这笔账**躲不开**：
/// - `CoUninitialize` 不卸载 DLL，它只动 COM 引用计数；
/// - `CoFreeUnusedLibraries` 也只回收真正实现了 `DllCanUnloadNow`、且此刻空闲的 COM
///   服务器，对这些系统组件一概无效。真去调它，只是给每个对话框多加一次全进程遍历；
/// - `IFileDialog` 并不比老式 `GetOpenFileNameW` 轻（两档实测 89 / 90 个模块、
///   +9.4 / +9.7 MB），"换成新 API 就清爽了"是不成立的期望。
///
/// 于是能做的就一件事：把这些页从**私有工作集**摘出去。私有提交回不来（DLL 还在），
/// 但任务管理器那一列会回落——用户眼里轻不轻，看的就是那一列。
fn settle_after_dialog() {
    crate::platform::working_set::request_trim_soon();
}

/// 原生「打开」对话框的公共实现（`GetOpenFileNameW`）。
///
/// `OFN_NOCHANGEDIR` 保证不会改动进程当前工作目录。
fn pick_open(
    filter: &str,
    title: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    let mut buf = vec![0u16; BUF_LEN];
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let title: Vec<u16> = title.encode_utf16().collect();
    let dir = dir_wide(start_dir);

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner_hwnd(owner),
        lpstrFilter: PCWSTR::from_raw(filter.as_ptr()),
        lpstrFile: PWSTR::from_raw(buf.as_mut_ptr()),
        nMaxFile: BUF_LEN as u32,
        lpstrTitle: PCWSTR::from_raw(title.as_ptr()),
        lpstrInitialDir: dir_ptr(&dir),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    settle_after_dialog();
    // `GetOpenFileNameW` 返回 `BOOL`（newtype，含 `as_bool`）：true = 选中，false = 取消/出错。
    if !ok.as_bool() {
        log_ext_error("打开对话框");
        return None;
    }
    buf_to_path(&buf)
}

/// 原生「另存为」对话框的公共实现（`GetSaveFileNameW`）。
///
/// * `default_name` —— 预填的文件名（不带扩展名，扩展名由 `lpstrDefExt` 自动补）。
/// * `start_dir` —— 初始目录；`None` = 交给系统记住的位置。
/// * `owner` —— 属主窗口，见模块头。
///
/// `OFN_OVERWRITEPROMPT`（重名先问一句）在裁剪那条路上是**最后的安全阀**：用户完全可以在
/// 对话框里挑一个已存在的文件，这一句确认是收尾的刹车。而"绝不能覆盖原片"这条硬约束不靠
/// 对话框保证 —— 它由 `core::gif_trim::rewrite_to` 自己挡掉（对话框可以被绕过，文件系统
/// 不能）。
fn pick_save(
    filter: &str,
    title: &str,
    def_ext: &str,
    default_name: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    let mut buf = vec![0u16; BUF_LEN];
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let title: Vec<u16> = title.encode_utf16().collect();
    // ★ `lpstrDefExt` 是 NUL 结尾串，必须自己补终止符。老常量（`"png\0"`）自带，
    // 但 AI 对话「保存回复」传的是动态推断的扩展名（如 "md"）——没补就会把堆上
    // 相邻内存读进来，用户看到 ".mdz" 这样的乱码后缀（2026-10-08 实测）。
    let def_ext: Vec<u16> = def_ext.encode_utf16().chain([0u16]).collect();
    let dir = dir_wide(start_dir);

    // 预填文件名：写进 `lpstrFile`（对话框把它当"初始文件名"）。
    let name: Vec<u16> = default_name.encode_utf16().collect();
    if name.len() + 1 > buf.len() {
        return None;
    }
    buf[..name.len()].copy_from_slice(&name);

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner_hwnd(owner),
        lpstrFilter: PCWSTR::from_raw(filter.as_ptr()),
        lpstrFile: PWSTR::from_raw(buf.as_mut_ptr()),
        nMaxFile: BUF_LEN as u32,
        lpstrTitle: PCWSTR::from_raw(title.as_ptr()),
        lpstrDefExt: PCWSTR::from_raw(def_ext.as_ptr()),
        lpstrInitialDir: dir_ptr(&dir),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    settle_after_dialog();
    if !ok.as_bool() {
        log_ext_error("另存为对话框");
        return None;
    }
    buf_to_path(&buf)
}

/// 弹出原生「打开」对话框，选一张图片（PNG 静态图或 GIF 动图；悬浮标用）。
pub fn open_image() -> Option<PathBuf> {
    pick_open(FILTER, TITLE, None, None)
}

/// 音效过滤器（击键音效「浏览…」用）——与 `platform::audio::SUPPORTED_EXT` 的
/// 常用子集对齐（mid/midi 这类"能播但没人当击键音效"的不单列，所有文件兜底）。
const FILTER_AUDIO: &str =
    "音效 (*.wav;*.mp3;*.wma;*.m4a;*.aac)\0*.wav;*.mp3;*.wma;*.m4a;*.aac\0所有文件 (*.*)\0*.*\0\0";

/// 击键音效「浏览…」的对话框标题。
const TITLE_AUDIO: &str = "选择击键音效\0";

/// 弹出原生「打开」对话框，选一段击键音效（击键音效「音效文件」行用）。
pub fn open_audio() -> Option<PathBuf> {
    pick_open(FILTER_AUDIO, TITLE_AUDIO, None, None)
}

/// 弹出原生「打开」对话框，选一个 GIF（裁剪窗「打开…」用）。
///
/// 只认 `*.gif`：裁剪链路要的是"有全局色表的动图"，选到 PNG 只会拿到一句
/// "这个 GIF 打不开"。把过滤器收窄，能让用户根本走不到那一步。
///
/// `start_dir` 传 **GIF 产物目录**（`temp/gif/`，"最近录的那些都在这儿"），
/// `owner` 传裁剪窗自己的句柄（真正模态于它，见模块头）。
pub fn open_gif(start_dir: Option<&Path>, owner: Option<isize>) -> Option<PathBuf> {
    pick_open(FILTER_GIF, TITLE_OPEN_GIF, start_dir, owner)
}

/// MP4 过滤器（视频时间裁剪「打开」用）——只认 `*.mp4`：裁剪链路吃的是本程序自己
/// 产出的 H.264+AAC 流，别的容器选进来只会拿到一句"打不开"。
const FILTER_MP4: &str = "视频 (*.mp4)\0*.mp4\0所有文件 (*.*)\0*.*\0\0";

/// MP4「打开」的对话框标题。
const TITLE_OPEN_MP4: &str = "选择要裁剪的视频\0";

/// MP4「保存」的对话框标题。
const TITLE_SAVE_MP4: &str = "裁剪后另存为\0";

/// 弹出原生「打开」对话框，选一个 MP4（时间裁剪快捷键"没录过"那条路用）。
///
/// `start_dir` 传 **MP4 产物目录**（`temp/record/`），`owner` 传 `None` 时对话框不模态于
/// 任何窗（快捷键这条路开窗之前弹，没有可模态的属主）。
pub fn open_mp4(start_dir: Option<&Path>, owner: Option<isize>) -> Option<PathBuf> {
    pick_open(FILTER_MP4, TITLE_OPEN_MP4, start_dir, owner)
}

/// 弹出原生「另存为」对话框，选一个 PNG 落点（贴图另存为用）。
pub fn save_png(default_name: &str) -> Option<PathBuf> {
    pick_save(
        FILTER_PNG,
        TITLE_SAVE_PNG,
        DEF_EXT_PNG,
        default_name,
        None,
        None,
    )
}

/// 截图 / 屏幕标注「另存为」的对话框标题。
///
/// 与 [`TITLE_SAVE_PNG`]（贴图另存为）分开：标题说清"正在存什么"是这类对话框唯一
/// 能传达上下文的地方 —— 用户此刻屏上可能已经什么都没有了（截图态的覆盖层已收）。
const TITLE_SAVE_SHOT: &str = "截图另存为\0";

/// 弹出原生「另存为」对话框，选一个 PNG 落点（**截图 / 屏幕标注**的保存用）。
///
/// 与 [`save_png`]（贴图另存为）只差标题，但两个入口刻意分开：写死一个"贴图另存为"
/// 的标题用在截图上，用户会以为自己点错了按钮。
///
/// `owner` 传 `None` 是**有意**的：调用方（覆盖层的通道回调）此刻正处在事件分发里，
/// 而属主窗口一旦误传野句柄会让对话框失败（见 [`owner_hwnd`]）。截图态收尾时覆盖层
/// 已经不再铺满屏幕，对话框不会被压住。
pub fn save_shot(
    default_name: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    pick_save(
        FILTER_PNG,
        TITLE_SAVE_SHOT,
        DEF_EXT_PNG,
        default_name,
        start_dir,
        owner,
    )
}

/// 弹出原生「另存为」对话框，选一个 GIF 落点（裁剪窗「保存」用）。
///
/// `start_dir` 传**原片所在目录**：裁剪多半是"刚录完就地看"，初始目录落在原片旁边，
/// 用户少点两层。预填名由调用方给（`xxx_裁剪`），所以默认不会撞上原片。
pub fn save_gif(
    default_name: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    pick_save(
        FILTER_GIF,
        TITLE_SAVE_GIF,
        DEF_EXT_GIF,
        default_name,
        start_dir,
        owner,
    )
}

/// MP4「另存为」对话框（视频时间裁剪「保存」用）。落点由用户挑，原片只读。
pub fn save_mp4(
    default_name: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    pick_save(
        FILTER_MP4,
        TITLE_SAVE_MP4,
        "mp4",
        default_name,
        start_dir,
        owner,
    )
}

/// 任意文件过滤器（AI 回复落盘用——产出扩展名种类多，不穷举；默认扩展名由调用方给）。
const FILTER_ANY: &str = "所有文件 (*.*)\0*.*\0\0";

/// AI 对话「保存回复…」的对话框标题。
const TITLE_SAVE_REPLY: &str = "保存 AI 回复\0";

/// 弹出原生「另存为」对话框，把一段文本落盘（AI 对话「保存回复…」用）。
///
/// 与图形产物（png/gif/mp4）不同：产出是文本，扩展名由调用方按内容推断（lua/json/md…），
/// 过滤器放宽为「所有文件」，只在文件名不带扩展名时用 `def_ext` 补全。
pub fn save_text(
    def_ext: &str,
    default_name: &str,
    start_dir: Option<&Path>,
    owner: Option<isize>,
) -> Option<PathBuf> {
    pick_save(
        FILTER_ANY,
        TITLE_SAVE_REPLY,
        def_ext,
        default_name,
        start_dir,
        owner,
    )
}

/// 弹出原生「选择文件夹」对话框（Vista+ 的 `IFileOpenDialog`）。
///
/// 返回选中目录的绝对路径；取消或失败返回 `None`。
///
/// ★ 2026-09-17 起**没有调用方**：面板不再有「保存目录」设置项（产物统一落
/// `temp/<模块>/`）。保留为平台能力——贴图的「另存为」若要选默认目录可直接复用。
///
/// 用 `IFileDialog` 而不是 `SHBrowseForFolder`：后者是 XP 时代的老对话框，没有地址栏、
/// 也不能粘贴路径，而两者都是系统自带、体积代价为零。
/// `CoInitializeEx` 用 `COINIT_APARTMENTTHREADED`：对话框自带消息泵，STA 是它的前提。
pub fn open_folder() -> Option<PathBuf> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };

    // 对话框是模态的、会阻塞：`defer_blocking` 里跑，别从窗口过程直接调。
    unsafe {
        // 已经初始化过会返回 RPC_E_CHANGED_MODE / S_FALSE，都不是致命错误——
        // 对话框照样能开，故只看是否成功创建实例。
        // 只在我们**自己**初始化成功时才配对 CoUninitialize：若线程已被别人以别的模式
        // 初始化（返回 RPC_E_CHANGED_MODE），那次引用计数不是我们加的，绝不能去减。
        let co_init_ok = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        struct ComGuard(bool);
        impl Drop for ComGuard {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }
        let _guard = ComGuard(co_init_ok);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
        dlg.SetOptions(FOS_PICKFOLDERS).ok()?;
        // `Show` 返回 Err 就是用户取消：那也要收尾——shell 全家桶照样已经加载进来了。
        let shown = dlg.Show(None);
        settle_after_dialog();
        shown.ok()?;
        let item = dlg.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok()?;
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个过滤器都必须以双 NUL 收尾（`描述\0通配符\0…\0\0`）——少一个 0 就会让对话框
    /// 把后面的内存当成过滤器继续读，表现为"下拉里出现乱码项"这种没法调试的现象。
    ///
    /// ★ 判据用**分段**而不是"数 NUL 的奇偶"：`描述\0通配符\0` 重复 n 遍 + 末尾双 NUL
    ///   一共 `2n+1` 个 NUL —— **恒为奇数**。写成"偶数"会把每一份正确的过滤器都判红
    ///   （这个错我在第一次写这条门禁时就踩了）。分段后 `描述/通配符` 是 2n 段、再加末尾
    ///   两个空段，段数 `2n+2` 才是**偶数**，而且"末尾两段为空"直接对应"双 NUL 结尾"。
    #[test]
    fn every_filter_is_well_formed_and_double_nul_terminated() {
        for (name, f) in [
            ("FILTER", FILTER),
            ("FILTER_PNG", FILTER_PNG),
            ("FILTER_GIF", FILTER_GIF),
        ] {
            assert!(f.ends_with('\0'), "{name} 没有以 NUL 收尾（末尾那个空段）");
            let segs: Vec<&str> = f.split('\0').collect();
            assert!(
                segs.len() >= 4,
                "{name} 至少要有「描述/通配符」一对，实得 {segs:?}"
            );
            assert_eq!(
                segs.len() % 2,
                0,
                "{name} 的段数不是偶数，说明描述与通配符没成对：{segs:?}"
            );
            // 末尾恰两个空段 = 双 NUL 结尾。
            assert_eq!(
                &segs[segs.len() - 2..],
                &["", ""],
                "{name} 不是双 NUL 结尾：{segs:?}"
            );
            // 中间不得有空段：空描述会让下拉里出现一条没名字的项，空通配符会让它什么都匹配不到。
            assert!(
                segs[..segs.len() - 2].iter().all(|s| !s.is_empty()),
                "{name} 的描述/通配符里有空段：{segs:?}"
            );
        }
    }

    /// 打开/另存为要用的标题与默认扩展名，同样必须带终止 NUL。
    #[test]
    fn every_title_and_def_ext_is_nul_terminated() {
        for (name, s) in [
            ("TITLE", TITLE),
            ("TITLE_SAVE_PNG", TITLE_SAVE_PNG),
            ("TITLE_SAVE_SHOT", TITLE_SAVE_SHOT),
            ("TITLE_OPEN_GIF", TITLE_OPEN_GIF),
            ("TITLE_SAVE_GIF", TITLE_SAVE_GIF),
            ("DEF_EXT_PNG", DEF_EXT_PNG),
            ("DEF_EXT_GIF", DEF_EXT_GIF),
        ] {
            assert_eq!(s.chars().last(), Some('\0'), "{name} 没带终止 NUL");
            // 只有一个 NUL（在末尾）——中间夹 NUL 会让标题在界面上截断。
            assert_eq!(s.matches('\0').count(), 1, "{name} 中间混进了 NUL");
        }
    }

    /// `*.gif` 过滤器只该出现一次，且不能把 png 也放进来：裁剪窗只吃 GIF。
    #[test]
    fn gif_filter_admits_only_gif() {
        assert!(FILTER_GIF.contains("*.gif\0"));
        assert!(!FILTER_GIF.contains("*.png"));
        // 与悬浮标那张（PNG+GIF 通吃）必须是**两份**，不能共用。
        assert_ne!(FILTER_GIF, FILTER);
    }

    /// 对话框回填的缓冲区读回路径：截到终止 NUL；退化输入不 panic。
    #[test]
    fn buf_to_path_truncates_at_the_first_nul() {
        let pathed: Vec<u16> = "C:\\a\\b.gif".encode_utf16().chain([0u16]).collect();
        assert_eq!(buf_to_path(&pathed), Some(PathBuf::from("C:\\a\\b.gif")));

        // 终止 NUL 之后的垃圾字节必须被丢掉（对话框可能保留上一次的长路径）。
        let dirty: Vec<u16> = "C:\\x.gif"
            .encode_utf16()
            .chain([0u16])
            .chain("\\stale\\junk".encode_utf16())
            .collect();
        assert_eq!(buf_to_path(&dirty), Some(PathBuf::from("C:\\x.gif")));

        // 用户取消时缓冲区是空的（首字节就是 NUL）→ None，而不是空字符串路径。
        assert_eq!(buf_to_path(&[0u16, 0, 0]), None);
        assert_eq!(buf_to_path(&[]), None);
        // 没有 NUL 的畸形输入也要能读完，不 panic（用整个缓冲区）。
        let no_nul: Vec<u16> = "C:\\c.gif".encode_utf16().collect();
        assert_eq!(buf_to_path(&no_nul), Some(PathBuf::from("C:\\c.gif")));
    }

    /// 初始目录寄存器：给了目录必然是「UTF-16 + 一个终止 NUL」；没给则是**空 Vec**
    /// （→ `null` 指针），不是"只含一个 NUL 的串"。
    #[test]
    fn initial_dir_is_wide_nul_terminated_or_empty() {
        let d = dir_wide(Some(Path::new("C:\\a\\b")));
        assert_eq!(d.last(), Some(&0u16), "初始目录没有终止 NUL");
        assert_eq!(d.iter().filter(|&&c| c == 0).count(), 1, "中间混进了 NUL");
        assert_eq!(String::from_utf16_lossy(&d[..d.len() - 1]), "C:\\a\\b");

        // ★ 空 `Vec` → `PCWSTR::null()`。若这里改成写一个空串，系统会把它当"目录名是空
        //   字符串"从而把对话框开到一处莫名其妙的目录。
        assert!(
            dir_wide(None).is_empty(),
            "没给目录时必须是空 Vec 而不是空串"
        );
        assert!(dir_ptr(&[]).is_null());
        assert!(!dir_ptr(&d).is_null());
    }

    /// 非 ASCII 路径（中文目录名）也要能原样编码——本项目的产物目录名全是中文。
    #[test]
    fn initial_dir_encodes_non_ascii() {
        let d = dir_wide(Some(Path::new("E:\\录制\\gif")));
        let s = String::from_utf16_lossy(&d[..d.len() - 1]);
        assert_eq!(s, "E:\\录制\\gif");
    }

    /// 属主句柄：`None` 与 `0` 都必须是"没有属主"（空 HWND），而不是一个野句柄。
    ///
    /// ★ 这条守的是"对话框到底模态于谁"。传了野句柄，`GetOpenFileNameW` 要么失败要么
    ///   把别人的窗口禁用掉；传空则对话框不属于任何人（裁剪窗上一版就是这么丢的）。
    #[test]
    fn owner_hwnd_maps_none_and_zero_to_null() {
        assert!(owner_hwnd(None).is_invalid());
        assert!(owner_hwnd(Some(0)).is_invalid());
        let h = owner_hwnd(Some(0x1234));
        assert!(!h.is_invalid());
        assert_eq!(h.0 as isize, 0x1234);
    }
}
