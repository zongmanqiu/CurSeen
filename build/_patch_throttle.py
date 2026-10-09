"""给 WGC 取帧加节流：把取帧频率从"DWM 推帧频率"压到"主循环的目标帧率"。

★ 为什么（2026-10-05 实测定位）：`FrameArrived` 每触发一次 `GetMessageW` 就返回，
   消息循环随即`TryGetNextFrame`。于是**取帧频率 = DWM 推帧频率**（显示器刷新率，
   60/120/144Hz 都可能），而主循环只要 30fps。
   多余的取帧不是白费的——它会让 DWM 在**同一个 `ID3D11Device`** 上做纹理拷贝，
   与主循环的 `CopyResource`（帧→`acq`）和 `copy_to_encode`（`acq`→编码设备）**抢 GPU**。
   抢的结果：Media Foundation 的 VideoProcessorMFT 拿不到资源、转换变慢、
   输入队列填满 ⇒ `ProcessInput` 返回 `MF_E_NOTACCEPTING`。

   实测数据（同一条链路、同样 30fps、开音频 + 6 条管线600s）：
   · DXGI 采集：忙拒收 **0** 次
   · WGC 采集（未节流）：忙拒收 0 / 7540 / 12142 / 17900 次
     ⇒ 最多那次 **18002 帧里 17900 帧被拒**（99.4%），视频轨只剩约 100 样本 / 3.3s，
       而 `mvhd` 被音频轨顶成 600s ⇒ **只看总时长会假绿**。
   ★ 每一帧被拒 = 容器里少一个样本（实测 7540 拒收 vs 7539 缺样本，逐个对应）。

⇒ 节流：两次取帧之间至少隔 `1/fps`，多出来的帧让`latest` 的"覆盖式"语义自然丢弃。
  这**不是**丢帧 —— 主循环本来就按 `frame_deadline` 定速，30fps 只需要 30 帧/秒。
跑完即删。
"""
P = 'src/platform/wgc_capture.rs'
raw = open(P, encoding='utf-8', newline='').read()

# ① 采集线程记住上次取帧时刻
A1 = '''    // ⑥ 消息循环。**没有它回调不会被派发**（`GetMessageW` 阻塞不占 CPU）。
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();'''
B1 = '''    // ⑥ 消息循环。**没有它回调不会被派发**（`GetMessageW` 阻塞不占 CPU）。
    //
    // ★★ 取帧要**节流到目标帧率**：否则取帧频率 = DWM 推帧频率（显示器刷新率），
    //   而主循环只要 30fps。多余的取帧会让 DWM 在**同一个 `ID3D11Device`** 上拷纹理，
    //   与主循环的 `CopyResource` / `copy_to_encode` 抢 GPU ⇒ VideoProcessorMFT 变慢
    //   ⇒ 输入队列填满 ⇒ `ProcessInput` 报`MF_E_NOTACCEPTING`。
    //   实测：未节流时 18002 帧里最多 17900 帧被拒（视频轨只剩约 100 样本 / 3.3s），
    //   而 `mvhd` 被音频轨顶成 600s ⇒ 只看总时长会**假绿**。
    //   `target_fps` 给 0 表示不限速（调用方明确要原始帧率时才用）。
    let min_gap = if target_fps == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(1_000_000_000 / target_fps.max(1) as u64)
    };
    let mut last_take: Option<Instant> = None;
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();'''
assert raw.count(A1) == 1, ('A1', raw.count(A1))
raw = raw.replace(A1, B1, 1)

# ② 取帧前加节流判断
A2 = '''        // ⑦ 取一帧（**非阻塞**：没帧就是"桌面没变"，不是错误）。
        let Ok(frame) = pool.TryGetNextFrame() else {
            continue;
        };'''
B2 = '''        // ⑦ 取一帧（**非阻塞**：没帧就是"桌面没变"，不是错误）。
        //    ★ 先节流：距上次取帧不足一个帧间隔就跳过本轮（这一帧已被
        //      `latest` 的"覆盖式"语义自然丢弃，主循环按 `frame_deadline` 定速，
        //      不需要更密的帧）。
        let t_now = Instant::now();
        if let Some(prev) = last_take {
            if t_now.duration_since(prev) < min_gap {
                continue;
            }
        }
        last_take = Some(t_now);
        let Ok(frame) = pool.TryGetNextFrame() else {
            continue;
        };'''
assert raw.count(A2) == 1, ('A2', raw.count(A2))
raw = raw.replace(A2, B2, 1)

# ③ open / run_session 传 target_fps
A3 = '''    pub fn open(device: ID3D11Device, monitor: Option<SendMonitor>) -> Result<Self, WgcError> {'''
B3 = '''    /// `target_fps` 是**主循环的录制帧率**（`RecOptions::fps`），用来给取帧节流。
    /// 传 0 = 不限速（只在"要拿到原始帧率"的诊断场景用）。
    pub fn open(
        device: ID3D11Device,
        monitor: Option<SendMonitor>,
        target_fps: u32,
    ) -> Result<Self, WgcError> {'''
assert raw.count(A3) == 1, ('A3', raw.count(A3))
raw = raw.replace(A3, B3, 1)

A4 = '''            match run_session(s_thread, device, monitor, &tx) {'''
B4 = '''            match run_session(s_thread, device, monitor, target_fps, &tx) {'''
assert raw.count(A4) == 1, ('A4', raw.count(A4))
raw = raw.replace(A4, B4, 1)

A5 = '''fn run_session(
    s: Arc<Shared>,
    dev: ID3D11Device,
    monitor: Option<SendMonitor>,
    tx: &std::sync::mpsc::Sender<std::result::Result<u32, WgcError>>,
) -> Result<(), WgcError> {'''
B5 = '''fn run_session(
    s: Arc<Shared>,
    dev: ID3D11Device,
    monitor: Option<SendMonitor>,
    target_fps: u32,
    tx: &std::sync::mpsc::Sender<std::result::Result<u32, WgcError>>,
) -> Result<(), WgcError> {'''
assert raw.count(A5) == 1, ('A5', raw.count(A5))
raw = raw.replace(A5, B5, 1)

open(P, 'w', encoding='utf-8', newline='').write(raw)
print('取帧节流已加')
