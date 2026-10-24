/* vphone-linux: restore boot-args plus VPHONE_RESTORE_BOOT_ARGS_EXTRA (diagnostics, e.g. serial=3). */
#ifndef VPHONE_LINUX_BOOTARGS_H
#define VPHONE_LINUX_BOOTARGS_H
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static inline char *vphone_linux_boot_args(const char *base)
{
    const char *extra = getenv("VPHONE_RESTORE_BOOT_ARGS_EXTRA");
    if (!extra || !*extra) {
        return strdup(base);
    }
    size_t n = strlen(base) + strlen(extra) + 2;
    char *s = malloc(n);
    if (s) {
        snprintf(s, n, "%s %s", base, extra);
    }
    return s;
}
#endif
