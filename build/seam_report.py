"""分段录制的**接缝报告** —— 回答"合起来之后接缝处会怎样"。

用法：python build/seam_report.py <.parts目录> [设定录制秒数] [切换耗时ms]...

逐个解析 part_*.mp4，报告每段的视频/音频时长与样本数，并算出：
  ① 段与段是否等长（不等长 = 切换吞掉了时间）
  ② 每段内部音视频是否对齐（不对齐 = 接缝处声音超前/滞后）
  ③ 各段时长之和 vs 设定录制时长（**只报差，不归因** —— 见文末告警）

★ 它只读 moov，不碰像素，几秒就跑完。

★★ 切换耗时**只能从日志取**（grep "切换耗时"），本脚本反推不出来 —— 反推会把
    启动延迟、父进程计时误差、末段天然未满全算进切换头上（实测报 804ms vs 真实 322ms）。
"""

import glob
import os
import struct
import sys


def boxes(d, off, end):
    """遍历一层 box。"""
    while off < end - 8:
        if off + 8 > len(d):
            break
        sz = struct.unpack(">I", d[off : off + 4])[0]
        typ = d[off + 4 : off + 8].decode("latin1", "replace")
        hdr = 8
        if sz == 1:
            sz = struct.unpack(">Q", d[off + 8 : off + 16])[0]
            hdr = 16
        if sz < 8 or off + sz > len(d) or off + sz > end:
            break
        yield typ, off + hdr, off + sz
        off += sz


def find(d, typ, off, end):
    for t, a, b in boxes(d, off, end):
        if t == typ:
            return a, b
    return None


def parse(path):
    d = open(path, "rb").read()
    mo = None
    for t, a, b in boxes(d, 0, len(d)):
        if t == "moov":
            mo = (a, b)
            break
    if mo is None:
        return None  # 没索引 = 没收尾 = 这段废了

    tracks = []
    for t, a, b in boxes(d, mo[0], mo[1]):
        if t != "trak":
            continue
        mdia = find(d, "mdia", a, b)
        if not mdia:
            continue
        # ★ hdlr 在 trak/mdia 下，不在 trak 直接子层（第一次写错在这里，读出全 0）。
        kind = ""
        hl = find(d, "hdlr", mdia[0], mdia[1])
        if hl:
            kind = d[hl[0] + 8 : hl[1]].split(b"\x00")[0].decode("latin1", "replace")
        mdhd = find(d, "mdhd", mdia[0], mdia[1])
        ts = dur = 0
        if mdhd:
            ver = d[mdhd[0]]
            if ver == 0:
                ts, dur = struct.unpack(">II", d[mdhd[0] + 12 : mdhd[0] + 20])
            else:
                ts, dur = struct.unpack(">IQ", d[mdhd[0] + 20 : mdhd[0] + 32])
        minf = find(d, "minf", mdia[0], mdia[1])
        stbl = find(d, "stbl", minf[0], minf[1]) if minf else None
        samples = 0
        if stbl:
            stsz = find(d, "stsz", stbl[0], stbl[1])
            if stsz:
                samples = struct.unpack(">I", d[stsz[0] + 8 : stsz[0] + 12])[0]
        tracks.append({"kind": kind, "ts": ts, "dur": dur, "n": samples})

    vid = next((t for t in tracks if t["kind"] == "vide"), None)
    aud = next((t for t in tracks if t["kind"] == "soun"), None)
    return {"bytes": len(d), "vid": vid, "aud": aud}


def secs(t):
    return t["dur"] / t["ts"] if t and t["ts"] else 0.0


def main():
    if len(sys.argv) < 2:
        sys.exit("用法：python build/seam_report.py <.parts目录> [实际录制秒数]")
    pdir = sys.argv[1]
    wall = float(sys.argv[2]) if len(sys.argv) > 2 else None
    # 第 3 个参数起 = 日志里的实际切换耗时（ms），可以逐个传，脚本自动求和。
    switch_ms = [float(x) for x in sys.argv[3:] if x.replace(".", "").isdigit()]

    files = sorted(glob.glob(os.path.join(pdir, "*.mp4")))
    if not files:
        sys.exit(f"目录里没有 mp4：{pdir}")

    print(f"分段目录 {pdir}（{len(files)} 段）")
    print()
    print(f"{'段':<14}{'视频样本':>9}{'视频时长':>10}{'音频样本':>9}{'音频时长':>10}{'A/V差':>9}  状态")
    print("─" * 78)

    vs, as_, rows = [], [], []
    for f in files:
        r = parse(f)
        name = os.path.basename(f)
        if r is None:
            print(f"{name:<14}{'—':>9}{'—':>10}{'—':>9}{'—':>10}{'—':>9}  ❌ 无 moov（未收尾）")
            continue
        v, a = secs(r["vid"]), secs(r["aud"])
        vn = r["vid"]["n"] if r["vid"] else 0
        an = r["aud"]["n"] if r["aud"] else 0
        gap = (a - v) * 1000 if (r["vid"] and r["aud"]) else 0.0
        vs.append(v)
        as_.append(a)
        rows.append((name, v, a))
        print(
            f"{name:<14}{vn:>9}{v:>9.3f}s{an:>9}{a:>9.3f}s{gap:>8.0f}ms  ✅ 可播"
        )

    if not vs:
        sys.exit("没有任何可播的段")

    print()
    print("──── 结论 ────")
    vmin, vmax = min(vs), max(vs)
    print(f"  视频段长 最短 {vmin:.3f}s / 最长 {vmax:.3f}s / 极差 {(vmax - vmin) * 1000:.0f} ms")
    if len(vs) > 1:
        # 去掉第一段（它含启动开销，不代表切换代价）
        rest = vs[1:]
        print(
            f"  第 2 段起 最短 {min(rest):.3f}s / 最长 {max(rest):.3f}s / 极差 "
            f"{(max(rest) - min(rest)) * 1000:.0f} ms"
        )
    print(f"  各段视频时长合计 {sum(vs):.3f}s")
    if wall:
        # ★ 别把「合计 vs 设定时长」的差**归因给段切换**（实测踩过：这里报 804ms，
        #   而日志里真实切换只有 166+156=322ms）。差里还混着：进程启动延迟、
        #   父进程计时误差、以及**最后一段本来就不满**（录 60s/段长 20s，末段天然短）。
        #   ⇒ 只报这个差，不做归因；真实切换耗时以日志「切换耗时 N ms」为准，
        #     或通过第 3 个参数逐个传入（自动求和）。
        lost = wall - sum(vs)
        print(f"  设定录制 {wall:.1f}s − 合计 {sum(vs):.3f}s = 差 {lost * 1000:.0f} ms")
        print("    ⚠ 这个差**不等于**切换耗时：它还含启动延迟 / 计时误差 / 末段天然未满。")
        if switch_ms:
            print(f"  实测切换耗时合计 {sum(switch_ms):.0f} ms（{len(switch_ms)} 次，"
                  f"平均 {sum(switch_ms) / len(switch_ms):.0f} ms）← 以这个为准")
        else:
            print("  实测切换耗时：查日志里的「切换耗时 N ms」（本脚本反推不出来）")
    worst = max(abs(a - v) for v, a in zip(vs, as_)) * 1000
    print(f"  段内音视频最大偏差 {worst:.0f} ms")

    print()
    print("  判读：")
    print("   · 段长极差小 ⇒ 分段没有打乱节奏，合并后时间轴连续（不会卡）。")
    print("   · 「切换吞掉」= 切换期间没采到的画面；它表现为内容少一点，**不是播放器卡住**。")
    print("   · A/V 偏差 > 100ms 才会被人耳察觉。")


if __name__ == "__main__":
    main()
