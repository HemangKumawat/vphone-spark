#!/bin/bash
# validate-run.sh RUN CID BUNDLE: boot with the Khronos Vulkan validation layer (shader and
# thread-safety checks off, errors only, to runs/RUN/validation.log), launch BUNDLE, and report
# validation errors and NVIDIA Xid faults seen since the launch.
cd "$(dirname "$0")/.."
RUN=$1; CID=$2; B=$3
mkdir -p runs/$RUN
export VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation
export VK_KHRONOS_VALIDATION_CHECK_SHADERS=false
export VK_KHRONOS_VALIDATION_THREAD_SAFETY=false
export VK_KHRONOS_VALIDATION_LOG_FILENAME=$PWD/runs/$RUN/validation.log
export VK_KHRONOS_VALIDATION_DEBUG_ACTION=VK_DBG_LAYER_ACTION_LOG_MSG
export VK_KHRONOS_VALIDATION_REPORT_FLAGS=error
export VK_KHRONOS_VALIDATION_DUPLICATE_MESSAGE_LIMIT=5
export M2V_VALIDATION_BACKTRACE=$PWD/runs/$RUN/validation-bt.txt
tools/boot-test.sh $RUN $CID
sleep 30
since=$(date '+%Y-%m-%d %H:%M:%S')
CID=$CID timeout 200 tools/vphoned.sh POST /v1/rpc \
  "{\"method\":\"apps.launch\",\"params\":{\"bundle_id\":\"$B\"},\"id\":1}" | head -c 100; echo
sleep 40
journalctl -k --since "$since" --no-pager 2>/dev/null | grep -i xid | tail -2
echo "validation errors: $(grep -c 'Validation Error' runs/$RUN/validation.log 2>/dev/null)"
