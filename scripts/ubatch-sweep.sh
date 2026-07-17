#!/bin/bash
# Ubatch sweep — 27B dense and 35B MoE, both backends, both pp sizes
# Results go to /tmp/ub-sweep-results.txt
# Each test has a 120s timeout, results include PASS/FAIL + t/s

OUT=/tmp/ub-sweep-results.txt
> $OUT
echo "=== Ubatch Sweep Results ===" >> $OUT
echo "Date: $(date)" >> $OUT
echo "" >> $OUT

# Helper function: run test with timeout
run_test() {
    local desc="$1"
    local binary="$2"
    local model="$3"
    local gpu_env="$4"
    local pp="$5"
    local b="$6"
    local ub="$7"
    local backend="$8"

    echo -n "[$desc] pp=$pp b=$b ub=$ub ... " >> $OUT

    # Use 120s timeout, capture output. Use env with proper var=value syntax.
    local result
    result=$(cd "$binary" && env LD_LIBRARY_PATH=. $gpu_env timeout 120 ./llama-bench \
        -m "$model" -p $pp -n 0 -b $b -ub $ub \
        -ctk q5_0 -ctv q4_1 -ngl 99 -fa on -r 1 -o jsonl 2>&1 | tail -1)

    if [ -z "$result" ]; then
        echo "FAIL (no output / timeout)" >> $OUT
        return 1
    fi

    # Check if output is JSON (starts with {)
    if [[ "$result" != \{* ]]; then
        echo "FAIL (not JSON): ${result:0:100}" >> $OUT
        return 1
    fi

    # Extract avg_ts from JSON
    local tps=$(echo "$result" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); print(f\"{d['avg_ts']:.2f}\")" 2>/dev/null)
    if [ -z "$tps" ]; then
        echo "FAIL (parse error): ${result:0:100}" >> $OUT
        return 1
    fi

    echo "${tps} t/s" >> $OUT
    return 0
}

# Models
M_27B=/mnt/models/unsloth/Qwen3.6-27B-MTP-GGUF/Qwen3.6-27B-UD-Q5_K_XL.gguf
M_MOE=/mnt/models/unsloth/qwen3.6-35b-a3b-mtp-gguf/Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf

# Binaries
VK_BIN=/root/.config/tama/backends/llama_cpp/vulkan/b10064
RC_BIN=/root/.config/tama/backends/llama_cpp/rocm/b9939

# ============ 27B Vulkan (GPU 0) ============
echo "### 27B DENSE — VULKAN (GPU 0) ###" >> $OUT
for pp in 2048 16384; do
    for ub in 1024 2048 4096 8192; do
        b=$ub
        run_test "27B-VK" "$VK_BIN" "$M_27B" "GGML_VK_VISIBLE_DEVICES=0" "$pp" "$b" "$ub" "vulkan"
    done
done

# ============ 27B ROCm (GPU 0) ============
echo "" >> $OUT
echo "### 27B DENSE — ROCm (GPU 0) ###" >> $OUT
for pp in 2048 16384; do
    for ub in 1024 2048 4096; do
        b=$ub
        run_test "27B-RC" "$RC_BIN" "$M_27B" "HIP_VISIBLE_DEVICES=0" "$pp" "$b" "$ub" "rocm"
    done
done

# ============ 35B MoE Vulkan (GPU 1) ============
echo "" >> $OUT
echo "### 35B MoE — VULKAN (GPU 1) ###" >> $OUT
for pp in 2048 16384; do
    for ub in 1024 4096 8192; do
        b=$ub
        run_test "MoE-VK" "$VK_BIN" "$M_MOE" "GGML_VK_VISIBLE_DEVICES=1" "$pp" "$b" "$ub" "vulkan"
    done
done

# ============ 35B MoE ROCm (GPU 1) ============
echo "" >> $OUT
echo "### 35B MoE — ROCm (GPU 1) ###" >> $OUT
for pp in 2048 16384; do
    for ub in 1024 2048 4096; do
        b=$ub
        run_test "MoE-RC" "$RC_BIN" "$M_MOE" "HIP_VISIBLE_DEVICES=1" "$pp" "$b" "$ub" "rocm"
    done
done

echo "" >> $OUT
echo "=== SWEEP DONE: $(date) ===" >> $OUT
echo "DONE"
