"""解析 MP4 的 moov，逐轨报告时长/样本数/采样间隔 —— 定位"帧数对但容器时长短"。

用法：python build/mp4_dur.py <mp4路径>
"""
import struct
import sys

path = sys.argv[1]
d = open(path, 'rb').read()
print(f'文件 {path}｜{len(d)} 字节')


def walk(off, end, depth=0, out=None):
    if out is None:
        out = []
    while off < end - 8:
        if off + 8 > len(d):
            break
        sz = struct.unpack('>I', d[off:off + 4])[0]
        typ = d[off + 4:off + 8].decode('latin1', 'replace')
        hdr = 8
        if sz == 1:
            if off + 16 > len(d):
                break
            sz = struct.unpack('>Q', d[off + 8:off + 16])[0]
            hdr = 16
        if sz < 8 or off + sz > len(d) or off + sz > end:
            break
        pad = '  ' * depth
        if typ in ('trak', 'mdia', 'minf', 'stbl'):
            walk(off + hdr, off + sz, depth + 1, out)
        elif typ == 'mvhd':
            ver = d[off + hdr]
            if ver == 0:
                ts, dur = struct.unpack('>II', d[off + hdr + 12:off + hdr + 20])
            else:
                ts, dur = struct.unpack('>IQ', d[off + hdr + 20:off + hdr + 32])
            out.append(f'{pad}mvhd timescale={ts} duration={dur} = **{dur / ts:.2f}s**')
        elif typ == 'mdhd':
            ver = d[off + hdr]
            if ver == 0:
                ts, dur = struct.unpack('>II', d[off + hdr + 12:off + hdr + 20])
            else:
                ts, dur = struct.unpack('>IQ', d[off + hdr + 20:off + hdr + 32])
            out.append(f'{pad}mdhd timescale={ts} duration={dur} = **{dur / ts:.2f}s**')
        elif typ == 'hdlr':
            name = d[off + hdr + 8:off + sz].split(b'\x00')[0].decode('latin1', 'replace')
            out.append(f'{pad}hdlr = {name}')
        elif typ == 'stts':
            cnt = struct.unpack('>I', d[off + hdr + 4:off + hdr + 8])[0]
            ent = [struct.unpack('>II', d[off + hdr + 8 + i * 8:off + hdr + 16 + i * 8])
                   for i in range(min(cnt, 4))]
            tot = sum(c * s for c, s in ent)
            out.append(f'{pad}stts 条目={cnt} 前几项(sample_count,delta)={ent} '
                       f'前{len(ent)}项合计={tot}')
        elif typ == 'stsz':
            cnt = struct.unpack('>I', d[off + hdr + 8:off + hdr + 12])[0]
            out.append(f'{pad}★ stsz 样本数 = {cnt}')
        off += sz
    return out


# 找 moov
off = 0
moov = None
while off < len(d) - 8:
    sz = struct.unpack('>I', d[off:off + 4])[0]
    typ = d[off + 4:off + 8].decode('latin1', 'replace')
    if sz == 1:
        sz = struct.unpack('>Q', d[off + 8:off + 16])[0]
    if sz < 8:
        break
    if typ == 'moov':
        moov = (off, sz)
        print(f'moov 在 off={off} size={sz}')
        break
    off += sz

if moov is None:
    sys.exit('★ 没有 moov！产物不完整')

for line in walk(moov[0] + 8, moov[0] + moov[1]):
    print(line)
