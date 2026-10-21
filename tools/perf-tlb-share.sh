#!/bin/bash
# perf-tlb-share.sh PERF.DATA: share of samples in softmmu TLB-fill / page-walk symbols, plus top 25.
f=${1:?perf.data}
perf report -i "$f" --no-children --sort sym --stdio -n 2>/dev/null | grep -v '^#' | grep '%' >"$f.syms"
python3 -I - "$f.syms" ${2:-25} <<'PY'
import sys, re
cats = {
 'fill/walk': re.compile(r'get_phys_addr|S1_ptw|S2_ptw|ptw_|_ptw\b|arm_cpu_tlb_fill|tlb_set_page|probe_access|'
                        r'tlb_fill|victim_tlb|aa64_va_parameters|regime_|address_space_translate_for_iotlb|'
                        r'flatview_do_translate|get_page_addr_code|tlb_add_large|arm_mmu_idx|get_s1|get_s2|'
                        r'ptw_attrs|check_s2|apple_.*sprr|sprr_'),
 'flush': re.compile(r'tlb_flush|tlbi|flush_all_helper|tlb_mmu_flush|tlb_window|tlb_mmu_resize'),
 'mem-slowpath': re.compile(r'do_ld|do_st|mmu_lookup|helper_ld|helper_st|cpu_ld|cpu_st|store_helper|load_helper|io_prepare|notdirty'),
}
# hybrid CPU: one section per PMU (cpu_core, cpu_atom); sum sample counts across them
from collections import Counter
cnt = Counter()
for line in open(sys.argv[1]):
    m = re.match(r'\s*([\d.]+)%\s+(\d+)\s+\[(.)\]\s+(.*)', line)
    if m: cnt[m.group(4).strip()] += int(m.group(2))
tot = sum(cnt.values()); acc = {k: 0 for k in cats}
for s, n in cnt.items():
    for k, r in cats.items():
        if r.search(s): acc[k] += n; break
print(f"samples {tot}: " + ", ".join(f"{k} {100*v/tot:.1f}%" for k, v in acc.items()))
for s, n in cnt.most_common(int(sys.argv[2]) if len(sys.argv) > 2 else 25):
    print(f"  {100*n/tot:5.2f}%  {s[:90]}")
PY
