/* vphone-linux: BSD funopen(3) for glibc, built on fopencookie(3). */
#ifndef VPHONE_LINUX_FUNOPEN_COMPAT_H
#define VPHONE_LINUX_FUNOPEN_COMPAT_H
#include <stdio.h>
#ifdef __linux__
#include <sys/types.h>
FILE *funopen(const void *cookie, int (*readfn)(void *, char *, int), int (*writefn)(void *, const char *, int),
              off_t (*seekfn)(void *, off_t, int), int (*closefn)(void *));
#endif
#endif
