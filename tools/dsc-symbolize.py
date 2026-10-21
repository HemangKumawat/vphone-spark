#!/usr/bin/env python3
"""dsc-symbolize.py SAMPLES SLIDE_BASE [TOP]: attribute tools/cpu-sample.py user-mode samples to
iOS shared-cache images and functions, using the cache copied to dsc/. SLIDE_BASE is the
cache's load address this boot (a crash report's sharedCache.base). Functions are named by the
nearest preceding symbol of the image's LC_SYMTAB (exported and kept local symbols)."""
import bisect, collections, os, struct, sys
D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "dsc")
samples, base = sys.argv[1], int(sys.argv[2], 0)
top = int(sys.argv[3]) if len(sys.argv) > 3 else 30
files = sorted(f for f in os.listdir(D) if f.startswith("dyld_shared_cache_arm64e"))
maps = []  # (addr, size, fileoff, file)
for f in files:
    with open(os.path.join(D, f), "rb") as fh:
        h = fh.read(0x4000)
    if h[:4] != b"dyld":
        continue
    mo, mc = struct.unpack_from("<II", h, 16)
    for i in range(mc):
        a, sz, fo, _, _ = struct.unpack_from("<QQQII", h, mo + i * 32)
        maps.append((a, sz, fo, f))
maps.sort()
mfh = {}
def read(addr, n):
    for a, sz, fo, f in maps:
        if a <= addr < a + sz:
            if f not in mfh: mfh[f] = open(os.path.join(D, f), "rb")
            fh = mfh[f]; fh.seek(fo + addr - a); return fh.read(n)
    return None
main = open(os.path.join(D, "dyld_shared_cache_arm64e"), "rb").read()
unslid = maps[0][0] if maps else 0x180000000
mo = struct.unpack_from("<I", main, 16)[0]
unslid = struct.unpack_from("<Q", main, mo)[0]
slide = base - unslid
io, ic = struct.unpack_from("<II", main, 0x1C0)
imgs = []
for i in range(ic):
    a, _, _, po, _ = struct.unpack_from("<QQQII", main, io + i * 32)
    imgs.append((a, main[po:main.index(b"\0", po)].decode()))
imgs.sort(); starts = [a for a, _ in imgs]
symcache = {}
def symbols(img_addr):
    if img_addr in symcache: return symcache[img_addr]
    hdr = read(img_addr, 0x8000) or b""
    out = []
    try:
        ncmds = struct.unpack_from("<I", hdr, 16)[0]; off = 32; link = None; symtab = None
        for _ in range(ncmds):
            cmd, size = struct.unpack_from("<II", hdr, off)
            if cmd == 0x19:  # LC_SEGMENT_64
                name = hdr[off + 8:off + 24].rstrip(b"\0")
                vm, vs, fo = struct.unpack_from("<QQQ", hdr, off + 24)
                if name == b"__LINKEDIT": link = (vm, fo)
            elif cmd == 2:  # LC_SYMTAB
                symtab = struct.unpack_from("<IIII", hdr, off + 8)
            off += size
        if link and symtab:
            symoff, nsyms, stroff, strsize = symtab
            lv, lf = link
            syms = read(lv + symoff - lf, nsyms * 16) or b""
            strs = read(lv + stroff - lf, strsize) or b""
            for i in range(len(syms) // 16):
                sx, ty, sect, desc, val = struct.unpack_from("<IBBHQ", syms, i * 16)
                if ty & 0x0E == 0x0E and val:  # N_SECT
                    e = strs.find(b"\0", sx); out.append((val, strs[sx:e].decode(errors="replace")))
    except struct.error:
        pass
    out.sort(); symcache[img_addr] = out; return out
rows = [l.split() for l in open(samples)]
img_c = collections.Counter(); fn_c = collections.Counter(); n = 0
for r in rows:
    for x in r[1:]:
        if x[0] != "U": continue
        n += 1
        pc = int(x[1:], 16) - slide
        i = bisect.bisect_right(starts, pc) - 1
        if i < 0 or pc - starts[i] > 0x10000000: img_c["(not in cache)"] += 1; continue
        name = imgs[i][1].split("/")[-1]; img_c[name] += 1
        s = symbols(starts[i]); j = bisect.bisect_right([v for v, _ in s], pc) - 1
        fn_c[f"{name}`{s[j][1] if j >= 0 else '?'}"] += 1
print(f"user samples {n}, slide {slide:#x}")
print("-- images"); [print(f"{v/n*100:5.1f}% {k}") for k, v in img_c.most_common(15)]
print("-- functions"); [print(f"{v/n*100:5.1f}% {k[:110]}") for k, v in fn_c.most_common(top)]
