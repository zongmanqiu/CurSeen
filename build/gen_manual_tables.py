#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""把 `API.md` 的接口表**机械镜像**进《Lua 脚本手册》第二部分。

为什么要有这个脚本（m23）：手册第二部分与 `API.md` 描述的是同一件事——同一批接口、同一批
元数据。过去两份各写各的，于是 m10–m19 开出去的 11 个 `ext:` 方法、3 个像素动作、
`requires` 与 `kind` 在全新文档里**一个都没有**（grep 实测），而没有任何机器读过它们的一致性。

处置：签名与名字这类**只该有一份**的事实，交给脚本从 `API.md` 生成；两份的"说明"栏
允许各写各的（手册写给人抄，`API.md` 写给人事先查），所以生成只覆盖这三张表的行，
一致性由 `tests/docs_consistency.rs::test_lua_manual_matches_the_api_reference_tables` 钉住
——**它读的是生成物的形状，不是这个脚本的自我描述**（脚本没跑、忘了跑，门禁就红）。

判据与门禁必须同一把尺子：表按**表头单元格序列**识别（不是按章节号，号一改就飘）。
"""
from __future__ import annotations

import io
import os
import sys

BS = chr(92)

# 与 Rust 门禁里的 header 一一对应；改这里必须同时改那里。
FAMILIES = [
    ["成员", "形式", "签名 / 返回", "说明"],
    ["全局名", "类型", "读取方", "说明"],
]


def split_md_row(line: str) -> list[str]:
    """按**未转义**的 `|` 切一格；单元格里的 `BS|` 是签名的一部分，不能当分隔符。"""
    cells: list[str] = []
    cur = ""
    esc = False
    for ch in line:
        if esc:
            cur += ch
            esc = False
            continue
        if ch == BS:
            cur += ch
            esc = True
            continue
        if ch == "|":
            cells.append(cur)
            cur = ""
            continue
        cur += ch
    cells.append(cur)
    out = []
    for c in cells:
        c = c.strip()
        if c:
            out.append(c)
    return out


def md_tables(text: str, header: list[str]) -> list[list[list[str]]]:
    """取所有表头等于 `header` 的表，返回 [表][行][列]。"""
    tables: list[list[list[str]]] = []
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        cells = split_md_row(lines[i])
        if len(cells) >= len(header) and cells[: len(header)] == header:
            rows: list[list[str]] = []
            i += 2  # 跳过分隔行
            while i < len(lines) and lines[i].lstrip().startswith("|"):
                c = split_md_row(lines[i])
                if len(c) >= len(header):
                    rows.append(c[: len(header)])
                i += 1
            tables.append(rows)
            continue
        i += 1
    return tables


def inline_md_to_html(cell: str) -> str:
    """`code` → <code>、**粗** → <strong>，其余按原样并转义 `&<>`。

    ★ 先转义再包标记：签名里的 `->` 必须变成 `-&gt;`（手册现有行就是这个形状），
      而 `` ` `` 与 `*` 不是 HTML 特殊字符，顺序反了会把 `&lt;code&gt;` 印出来。
    """
    t = cell.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    out = []
    i = 0
    while i < len(t):
        if t.startswith("<code>", i):
            i += 6
            continue
        if t.startswith("</code>", i):
            i += 7
            continue
        out.append(t[i])
        i += 1
    t = "".join(out)
    # markdown 的 `\|` 是表格转义，渲染出来是 `|`——手册里不该让用户看见那个反斜杠。
    t = t.replace(BS + "|", "|")
    # 反引号与星号都在转义之后仍是 ASCII，可以直接按标记切
    parts = t.split("`")
    seg: list[str] = []
    for idx, p in enumerate(parts):
        seg.append("<code>" + p + "</code>" if idx % 2 == 1 else p)
    t = "".join(seg)
    parts = t.split("**")
    seg = []
    for idx, p in enumerate(parts):
        seg.append("<strong>" + p + "</strong>" if idx % 2 == 1 else p)
    return "".join(seg)


def render_tbody(rows: list[list[str]]) -> str:
    body = ["<tbody>"]
    for r in rows:
        body.append("<tr>")
        for cell in r:
            body.append("<td>" + inline_md_to_html(cell) + "</td>")
        body.append("</tr>")
    body.append("</tbody>")
    return "\n".join(body)


def strip_tags(text: str) -> str:
    """去掉 `<...>` 片段（只用于读表头，产出文本不经过它）。"""
    out: list[str] = []
    skip = False
    for ch in text:
        if ch == "<":
            skip = True
        if not skip:
            out.append(ch)
        if ch == ">":
            skip = False
    return "".join(out)


def table_ths(block: str) -> list[str]:
    """取一张 `<table>` 的表头单元格文本（手册里 `<th>` 是**一行一个**，不能按整行字符串比）。"""
    out: list[str] = []
    rest = block
    while "<th>" in rest:
        rest = rest.split("<th>", 1)[1]
        if "</th>" not in rest:
            break
        out.append(strip_tags(rest.split("</th>", 1)[0]).strip())
    return out


def sync_html_tables(html: str, header: list[str], rows_per_table: list[list[list[str]]]) -> tuple[str, int]:
    """按出现顺序把表头等于 `header` 的所有 `<tbody>` 换成生成结果；返回新文本与替换数。"""
    out = html
    n = 0
    pos = 0
    while True:
        tstart = out.find("<table>", pos)
        if tstart < 0:
            break
        tend = out.find("</table>", tstart)
        if tend < 0:
            break
        block = out[tstart:tend]
        head_ok = table_ths(block) == header
        bstart = block.find("<tbody>")
        bend = block.find("</tbody>")
        if head_ok and bstart >= 0 and bend >= 0 and n < len(rows_per_table):
            new_block = (
                block[:bstart] + render_tbody(rows_per_table[n]) + block[bend + len("</tbody>"):]
            )
            out = out[:tstart] + new_block + out[tstart + len(block):]
            pos = tstart + len(new_block)
            n += 1
        else:
            pos = tstart + 8
    return out, n


def main() -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.dirname(here)  # main/
    api = os.path.join(root, "assets", "lua", "API.md")
    manual = os.path.join(root, "assets", "lua", "Lua脚本手册.html")
    md = io.open(api, encoding="utf-8").read()
    html = io.open(manual, encoding="utf-8").read()

    total = 0
    for header in FAMILIES:
        rows = md_tables(md, header)
        html, n = sync_html_tables(html, header, rows)
        if n != len(rows):
            print(
                "表族 {0} 在 API.md 里有 {1} 张、手册里只换成 {2} 张——"
                "两副面孔的形状已经不对，脚本拒绝瞎改".format(header, len(rows), n),
                file=sys.stderr,
            )
            return 2
        total += sum(len(r) for r in rows)
        print("同步表族 {0}: {1} 张 / {2} 行".format(header, len(rows), sum(len(r) for r in rows)))
    io.open(manual, "w", encoding="utf-8", newline="").write(html)
    print("OK 手册第二部分已镜像 API.md（共 {0} 行），文件：{1}".format(total, manual))
    return 0


if __name__ == "__main__":
    sys.exit(main())
