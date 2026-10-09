//! 屏幕页的抓取录制三卡：截图与贴图 + GIF录制 + 视频录制（**原「录制」页**）。
//!
//! ★ 2026-09-25 重排：分类判据从"按产物形态"改成"按操作对象"——截图（一帧）、GIF（多帧）、
//! 录屏（视频）操作的都是**屏幕画面**，与屏幕标注、放大镜同类，于是「录制」不再单独占一页，
//! 而是屏幕页内的三张平铺卡（页内不设分组标题）。本文件因此**不是一页**，
//! 只导出一个分区构造函数供屏幕页组装——`pages` 目录下唯一一个非页文件。
//!
//! ★ 每张卡标题栏都有**功能开关**（2026-09-25 用户口径）：关 = 该组快捷键全部不响应。
//! 开关之外快捷键为空串仍是第二道闸（不配键位 = 不可触发）。
//!
//! 为什么录屏没有"暂停"：DXGI 桌面复制 + Media Foundation 的 SinkWriter 是一条流式
//! 通道，暂停要额外维护时间轴偏移，而用户真正想要的只是"停掉这一段"。做成"按一下开始、
//! 再按一下停止并落盘"，语义简单、也不会录出半截文件。
//!
//! ★ 2026-09-17 精简（用户口径）：截图卡片只留**快捷键 + 贴图数量上限**。
//! 「上次选区截图」「保存目录」「存入剪贴板」「保存后提示」「监听剪贴板」「贴图窗描边」
//! 六项全部删除——前三个是功能下线（含各自的配置字段），后三个"删掉开关、行为恒开"。
//! 留档位置不再是设置项：程序自身的临时产物统一落 `temp/<模块>/`
//! （见 `core::paths::module_temp_dir`），用户要留下的走贴图右键「另存为」。
//!
//! # 三张卡片的分工（2026-09-18 重排）
//!
//! ```text
//! 截图与贴图      截图 / 截图并贴图 / 贴图 三把快捷键 + 贴图数量上限
//! GIF录制        录制（一键开关）+ 裁剪 / 帧率 / 录制鼠标（无声）
//! 视频录制        父卡（无开关，快捷键为空即停用）
//!     ├ 快捷键      录屏（一键开关）
//!     ├ 画面      帧率 / 分辨率 / 码率 / 硬件加速 / 录制鼠标
//!     └ 声音      系统声音（三态来源）/ 麦克风（三态来源 + 试听）/ 降噪
//! ```
//!
//! ★ 本页行标签不带「快捷键」二字（2026-09-26 晚用户口径）：卡片已经叫「GIF录制」，
//! 「视频录制 > 快捷键」子卡也已经说明用途，行尾再缀"快捷键"是废话。
//!
//! ★ 2026-09-27 口径更新：设置页「快捷键」集合的末段是**本页行名的逐字原文**——所以
//! 「截图与贴图」三行在集合里也与本页一字不差（「截图与贴图 > 截图」「截图与贴图 >
//! 截图后贴图」「截图与贴图 > 贴图」，见 `global::HOTKEY_ENTRIES`），而 GIF / 视频录制
//! 三行不带（「GIF录制 > 录制」）。差别不是两套命名，而是**原样搬运**：集合里写的就是
//! 这一行在页里的名字，用户按名字在两个页面之间对得上（由
//! `hotkey_collection_follows_the_ui_order_and_reuses_ui_text` 扫本文件盯住）。
//!
//! ★ 2026-09-27 二次口径：这三行**去掉「快捷键」三字**（「截图快捷键」→「截图」、
//! 「截图并贴图快捷键」→「截图后贴图」、「贴图快捷键」→「贴图」）。「截图与贴图」卡里
//! 三把键挨着的两行只读「截图」「贴图」就已经分得清，缀上"快捷键"反而是噪声——何况旁边
//! 「录制」「裁剪」那两行本来就不带。集合卡靠上面的原样搬运规则自动跟着变，不必手抄。
//!
//! ## 为什么「视频录制」要拆子卡，而「GIF录制」不拆
//!
//! 判据是**长度**（与键盘页虚拟键盘卡同一条理由）：录屏的参数面是 13 项，平铺一张卡
//! 翻不到底，且"快捷键 / 画面 / 声音"本来就是用户能自己回答的三个独立问题；GIF 只有 4 项
//! （录制快捷键 + 裁剪快捷键 + 帧率 + 录制鼠标），拆开每张子卡只剩一两行，反而要多点两次折叠。
//! ★ 2026-09-20：GIF 的「输出尺寸」「无限循环」两行已删——选框即尺寸，循环恒定开启
//! （见 `core::config::GifSettings` 的说明）。
//!
//! ## 为什么录制快捷键是「一键开关」（2026-09-19 用户口径）
//!
//! 原来是「开始键 + 停止键」两把。改成 toggle 的三个理由：① 少占一个全局快捷键位；
//! ② 两把功能重叠的键本身制造歧义（用户得记住"哪把是哪把"），而直播姬这类工具的通行做法
//! 就是一颗键开关；③ MP4 与 GIF 共用同一条 `RecOptions` 通道、同一个倒计时，一次只可能有
//! 一个录制在跑 —— 所以"任一把录制键在录制中按下 = 停掉当前这段"是自洽的。
//! 停止键字段（`record.stop_hotkey`）随之删除，不做兼容读入（旧配置里那行会被忽略）。
//!
//! ## 音频行的显隐
//!
//! 音频两路各只有一行三态下拉（关闭 / 默认 / 指定设备），"开关关着"这一态直接由「关闭」
//! 表达，因此不存在"开关关着、来源却还指着一台设备"的行。降噪说明只在降噪档 ≠「不降噪」
//! 时出现：那行在关着时**真的什么也不做**，摆在那就是在骗用户点。
//!
//! ★ 2026-09-22 修正：「麦克风试听」**不再随来源显隐**。此前它跟着
//! `visible_when(rec_mic_source > 0)` 走，麦克风一选「关闭」就只剩左边一个孤零零的标签，
//! 用户报"点击按钮没了"。现在恒显示，且来源是「关闭」时点「开始试听」会先把来源切到
//! 「跟随系统默认」（下拉同帧跟着变，不是静默改配置）——按钮在任何状态下都有反应。
//!
//! ★ 2026-09-26 晚出厂（用户口径）：**录屏 / GIF 录制 / 裁剪三把快捷键全部留空**，
//! 音频两路（系统声音 / 麦克风）都默认「跟随系统默认」、**麦克风降噪默认「中」**。
//! 快捷键留空 = 不占用用户的全局键位；音频默认开 + 默认降噪，是因为"录完才发现没声音
//! /全是底噪"不可逆，而"这一路我不需要"是一次下拉就能改的事。

use windui::prelude::*;

use crate::core::config::{
    GIF_FPS_LABELS, MIC_DENOISE_LABELS, PIN_CAP_LABELS, RECORD_BITRATE_LABELS, RECORD_FPS_LABELS,
    RECORD_HARDWARE_LABELS, RECORD_RESOLUTION_LABELS,
};
use crate::platform::audio_capture::DefaultDevice;
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{HotkeyField, PanelModel, AUDIO_SRC_DEFAULT, AUDIO_SRC_OFF};

/// 一行「标签 + 开关」。
fn toggle_row(label: &str, sig: Signal<bool>, col1_w: i32) -> Element {
    card::option_row(label, Element::checkbox("", sig), col1_w)
}

/// 一行「标签 + 下拉」（静态 `&str` 选项）。
fn choice_row(label: &str, options: &[&str], sig: Signal<usize>, col1_w: i32) -> Element {
    // 委托 `card` 正本：宽度（CONTROL_GROUP_W）与截断预算只在一处定义。
    card::choice_row(
        label,
        options.iter().map(|s| s.to_string()).collect(),
        sig,
        col1_w,
    )
}

/// 一行「标签 + 下拉」（运行时 `String` 选项，如音频设备来源列表）。
///
/// §37 起与静态选项同走 `card::choice_row` 正本：闭合框固定 `CONTROL_GROUP_W`、
/// 超宽当前项绘制时前置省略号；弹出菜单按内容自适应，设备名完整可辨——
/// 此前本函数特意保持内容宽（豁免），正是被「弹出层也被截断」逼出来的妥协，
/// windui 补丁落地后不再需要。
fn choice_row_str(label: &str, options: Vec<String>, sig: Signal<usize>, col1_w: i32) -> Element {
    card::choice_row(label, options, sig, col1_w)
}

/// 音频「来源」下拉的选项串：三态前缀（关闭 / 默认）+ 运行期枚举到的设备名。
///
/// ★ 下标契约定义在 `ui::model` 的 `AUDIO_SRC_*`：0 = 关闭，1 = 默认，2.. = `devices[i-2]`。
/// 这里只负责"按这个顺序造选项"，**不解释下标** —— 偏移量两处各写一遍就会错位。
///
/// `default` 有值时，「默认」带上**当前系统默认设备的简短名**（如
/// `默认 (Realtek High Definition Audio)`）：用户不必去系统设置里翻才知道跟随的是哪台。
/// 括号用 **ASCII 半角**（用户口径 2026-09-19）：里面装的是拉丁设备名，Windows 自己
/// 的 `PKEY_Device_FriendlyName` 与下面那串设备项用的也都是半角括号，混用全角会让
/// 同一列出现两种括号，看起来像两套文案拼起来的。
/// `devices` 里**已经不含**这台（`ui::model::drop_default_device` 剔掉了），所以不会
/// 出现两条指向同一端点的选项。
fn audio_source_options(devices: &[String], default: Option<&DefaultDevice>) -> Vec<String> {
    let mut opts = vec![
        "关闭".to_string(),
        match default {
            Some(d) => format!("默认 ({})", d.short),
            None => "默认".to_string(),
        },
    ];
    opts.extend(devices.iter().cloned());
    opts
}

/// 本分区用到的**全部标签**（卡片名 + 参数行标签）。
///
/// ★ 屏幕页算列宽时必须把它算进去：一页只有一个对齐基准（见 `card::option_row`），
///   漏掉这些标签会让本分区的控件列比页内其它卡窄一截——「贴图数量上限」「麦克风试听」
///   这类长标签最先露馅。
///
/// 只列**静态**标签：音频设备名是运行期枚举出来的，由调用方（`screen.rs`）自行 extend。
pub fn capture_labels() -> Vec<&'static str> {
    vec![
        "截图与贴图",
        "GIF录制",
        "视频录制",
        "快捷键",
        "画面",
        "声音",
        "截图",
        "截图后贴图",
        "贴图",
        "长截图",
        "宽截图",
        "贴图数量上限",
        "录制",
        "裁剪",
        "帧率",
        "录制鼠标",
        "录屏",
        "分辨率",
        "码率",
        "硬件加速",
        "系统声音",
        "麦克风",
        "麦克风试听",
        "麦克风降噪",
    ]
}

/// 组装屏幕页的抓取录制三卡：截图与贴图 / GIF录制 / 视频录制。
///
/// ★ `col1_w` **由屏幕页统一给**——本分区不再自己算列宽（原先「录制」是一整页，现在
/// 只是屏幕页里的一段），两处各算一个基准会让这三张卡与页内其它卡的控件列错位。
/// 返回值是**内容块**（一个 `col`），滚动容器与内边距由屏幕页统一包。
pub fn build_capture_section(m: &PanelModel, col1_w: i32) -> Element {
    let shot_expanded = m.expand("record.shot");
    let gif_expanded = m.expand("record.gif");
    let video_expanded = m.expand("record.video");
    let v_hotkey_expanded = m.expand("record.video.hotkey");
    let v_image_expanded = m.expand("record.video.image");
    let v_audio_expanded = m.expand("record.video.audio");

    let mut content = Element::col();

    // ── 截图与贴图 ──
    // ★ 开关在标题栏（2026-09-25 用户口径）：关 = 三把快捷键全部不响应（运行时按键时读
    //   `screenshot.enabled`，勾选即生效，无需重启/补帧——快捷键触发型功能没有常驻帧）。
    let (title, body, exp) = FeatureCard::new("截图与贴图", shot_expanded)
        .switch(m.shot_enabled)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "截图",
                    m.hk_shot,
                    HotkeyField::Shot,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "截图后贴图",
                    m.hk_shot_pin,
                    HotkeyField::ShotPin,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "贴图",
                    m.hk_pin_clipboard,
                    HotkeyField::PinClip,
                    m.hotkey_capture,
                    col1_w,
                ))
                // ★ 长截图（2026-10-01 新增，出厂留空）：一键开关，受本卡总开关管辖。
                .child(card::hotkey_row(
                    "长截图",
                    m.hk_shot_scroll,
                    HotkeyField::ShotScroll,
                    m.hotkey_capture,
                    col1_w,
                ))
                // ★ 宽截图（2026-10-01 新增，出厂留空）：同长截图，方向向右。
                .child(card::hotkey_row(
                    "宽截图",
                    m.hk_shot_scroll_wide,
                    HotkeyField::ShotScrollWide,
                    m.hotkey_capture,
                    col1_w,
                ))
                // ── 贴图（pin）──
                .child(choice_row(
                    "贴图数量上限",
                    &PIN_CAP_LABELS,
                    m.pin_cap,
                    col1_w,
                )),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── GIF录制 ──
    //
    // ★ 与录屏**共用采集管线**（`recorder::Session` 抓帧 + 缩放），差别只在封装器：
    //   MP4 走 MF SinkWriter（H.264 + AAC），GIF 走 `core::gif_enc`（调色板量化 + LZW）。
    //   所以这里没有"另一套抓屏代码"，也没有音频行 —— GIF 格式本身不支持音轨。
    let (title, body, exp) = FeatureCard::new("GIF录制", gif_expanded)
        .switch(m.gif_enabled)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "录制",
                    m.hk_gif,
                    HotkeyField::Gif,
                    m.hotkey_capture,
                    col1_w,
                ))
                // ★ 两把键**分开**而不是合成一把：录制是"框选并录"，裁剪是"打开编辑器"。
                //   合成的话，想裁磁盘上一个老 GIF 的用户就得先录点什么。
                .child(card::hotkey_row(
                    "裁剪",
                    m.hk_gif_trim,
                    HotkeyField::Trim,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(choice_row("帧率", &GIF_FPS_LABELS, m.gif_fps, col1_w))
                .child(toggle_row("录制鼠标", m.gif_cursor, col1_w)),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 视频录制（父卡 + 三张子卡：快捷键 / 画面 / 声音）──
    let (title, body, exp) = FeatureCard::new("视频录制", video_expanded)
        .switch(m.rec_enabled)
        .body({
            let mut col = Element::col();

            // ── 快捷键 ──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("快捷键", v_hotkey_expanded)
                .body(
                    Element::col()
                        .child(card::hotkey_row(
                            "录屏",
                            m.hk_record,
                            HotkeyField::Record,
                            m.hotkey_capture,
                            col1_w,
                        ))
                        .child(card::hotkey_row(
                            "选区录屏",
                            m.hk_record_region,
                            HotkeyField::RecordRegion,
                            m.hotkey_capture,
                            col1_w,
                        ))
                        .child(card::hotkey_row(
                            "时间裁剪",
                            m.hk_record_trim,
                            HotkeyField::RecordTrim,
                            m.hotkey_capture,
                            col1_w,
                        )),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));

            // ── 画面 ──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("画面", v_image_expanded)
                .body(
                    Element::col()
                        .child(choice_row("帧率", &RECORD_FPS_LABELS, m.rec_fps, col1_w))
                        .child(choice_row(
                            "分辨率",
                            &RECORD_RESOLUTION_LABELS,
                            m.rec_resolution,
                            col1_w,
                        ))
                        .child(choice_row(
                            "码率",
                            &RECORD_BITRATE_LABELS,
                            m.rec_bitrate,
                            col1_w,
                        ))
                        .child(choice_row(
                            "硬件加速",
                            &RECORD_HARDWARE_LABELS,
                            m.rec_hardware,
                            col1_w,
                        ))
                        .child(toggle_row("录制鼠标", m.rec_cursor, col1_w)),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));

            // ── 声音 ──
            //
            // 顺序 = 用户的心智顺序：先决定"录不录这一路"，再决定"从哪个设备来"，
            // 最后才是"要不要处理它"。麦克风的处理链（试听 / 降噪 / 强度）跟在麦克风
            // 之后，因为降噪只作用于麦克风，放在系统声音旁边会让人以为它对两路都生效。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("声音", v_audio_expanded)
                .body({
                    let audio = Element::col()
                        .child(choice_row_str(
                            "系统声音",
                            audio_source_options(
                                &m.rec_speaker_devices,
                                m.rec_speaker_default.as_ref(),
                            ),
                            m.rec_speaker_source,
                            col1_w,
                        ))
                        .child(choice_row_str(
                            "麦克风",
                            audio_source_options(&m.rec_mic_devices, m.rec_mic_default.as_ref()),
                            m.rec_mic_source,
                            col1_w,
                        ))
                        .child(
                            // ★ 这一行必须走 `option_row`（而不是裸 `row`）：否则按钮的左边缘
                            //   与其它行的控件列差一个 `col1_w`，整张卡的对齐基准就破了。
                            //
                            // ★★ 2026-09-22 用户报"麦克风试听，点击按钮没了"——根因是这一行
                            //   此前带 `visible_when(rec_mic_source > 0)`：麦克风一选「关闭」，
                            //   按钮整条消失，只剩左边一个孤零零的标签，看着像"按钮被改坏了"。
                            //   现在**恒显示**：关闭时点「开始试听」会先把来源切到「跟随系统默认」
                            //   （上面的下拉会立刻跟着变，不是静默改配置），再开始试听 ——
                            //   按钮在任何来源状态下都有反应，且不存在"试听在响、停止按钮却不在"
                            //   的残局（运行时的 `want` 仍要求来源非关闭，见 `sync_mic_monitor`）。
                            card::option_row(
                                "麦克风试听",
                                {
                                    // 试听 = 一条真实的 WASAPI 采集→播放直通链（见
                                    // `audio_capture::MicMonitor`），走的是与录制同一条降噪链，
                                    // 所以听到的就是录进去的效果。两个按钮按状态互斥显示：
                                    // 一个控件只有一个"外观"，而"开始试听/停止试听"必须是两个不同的
                                    // 视觉反馈（否则用户不知道现在到底在不在听）。
                                    let on = m.rec_monitor;
                                    let src = m.rec_mic_source;
                                    let play = on;
                                    let stop = on;
                                    Element::row()
                                        .child(
                                            Element::button("开始试听")
                                                .on_click(move |_ctx| {
                                                    // 来源是「关闭」时先补上「默认」：用户点这个按钮
                                                    // 的意图就是"想听麦克风"，让他先去改下拉是白绕一圈；
                                                    // 下拉在同一帧跟着变，状态变化是可见的。
                                                    if src.get() == AUDIO_SRC_OFF {
                                                        src.set(AUDIO_SRC_DEFAULT);
                                                    }
                                                    play.set(true)
                                                })
                                                .visible_when(move || !on.get()),
                                        )
                                        .child(
                                            Element::button("停止试听")
                                                .on_click(move |_ctx| stop.set(false))
                                                .visible_when(move || on.get()),
                                        )
                                },
                                col1_w,
                            ),
                        )
                        .child(choice_row(
                            "麦克风降噪",
                            &MIC_DENOISE_LABELS,
                            m.rec_denoise,
                            col1_w,
                        ));
                    audio
                })
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));

            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    content
}

#[cfg(test)]
mod tests {
    use super::audio_source_options;
    use crate::platform::audio_capture::DefaultDevice;

    /// ★ 音频来源下拉的**选项顺序**就是下标契约，顺序错了不会报错、只会选错设备。
    ///
    /// 契约（与 `ui::model::AUDIO_SRC_*` 同源）：`0 = 关闭`、`1 = 默认`、`2.. = 设备[i-2]`。
    /// 这条用例把"选项串"和"下标偏移"钉在一起 —— 任何一边单独改动都会红。
    #[test]
    fn audio_source_options_follow_the_index_contract() {
        let devs = vec!["麦克风 A".to_string(), "麦克风 B".to_string()];
        let opts = audio_source_options(&devs, None);
        assert_eq!(opts.len(), 4, "应为 关闭 + 默认 + 2 台设备");
        assert_eq!(opts[0], "关闭", "下标 0 必须是「关闭」");
        assert_eq!(opts[1], "默认", "下标 1 必须是「默认」");
        assert_eq!(opts[2], "麦克风 A", "下标 2 = 设备列表第 1 项");
        assert_eq!(opts[3], "麦克风 B", "下标 3 = 设备列表第 2 项");
        // 没有设备时也不能只剩一项 —— 否则用户找不到"关闭"这一态。
        let empty = audio_source_options(&[], None);
        assert_eq!(empty, vec!["关闭".to_string(), "默认".to_string()]);
    }

    /// 「默认」这一项必须写清"现在跟随的是哪台"，且**不能用完整名**。
    ///
    /// 完整名是 `扬声器 (Realtek High Definition Audio)`，外面那层 `扬声器 (...)` 外壳
    /// 是为了和"麦克风"区分；拼进「默认 (…)」会变成
    /// `默认 (扬声器 (Realtek High Definition Audio))`，把下拉撑宽、控件列跟着错位。
    #[test]
    fn default_option_names_the_current_device_without_the_wrapper() {
        let d = DefaultDevice {
            full: "扬声器 (Realtek High Definition Audio)".into(),
            short: "Realtek High Definition Audio".into(),
        };
        let opts = audio_source_options(&["耳机 (USB Audio)".to_string()], Some(&d));
        assert_eq!(opts[0], "关闭");
        assert_eq!(opts[1], "默认 (Realtek High Definition Audio)");
        assert_eq!(opts[2], "耳机 (USB Audio)", "设备项仍排在下标 2");
        assert!(
            !opts[1].contains("扬声器 ("),
            "「默认」里不能出现完整名的外壳，否则文案过长"
        );
    }
}
