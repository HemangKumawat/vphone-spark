/* vphone-linux: idevicerestore's device-event waits are sized for hardware-speed devices. Under
 * QEMU's emulation each boot stage (iBSS verifying and starting iBEC, the restore kernel coming
 * up) takes many times longer, so the waits are scaled: VPHONE_RESTORE_TIMEOUT_SCALE, default 10. */
#ifndef VPHONE_LINUX_TIMEOUT_H
#define VPHONE_LINUX_TIMEOUT_H
#include <stdlib.h>

static inline unsigned int vphone_linux_timeout(unsigned int ms)
{
    const char *env = getenv("VPHONE_RESTORE_TIMEOUT_SCALE");
    int scale = env && *env ? atoi(env) : 10;
    return ms * (unsigned int)(scale > 0 ? scale : 1);
}
#endif
