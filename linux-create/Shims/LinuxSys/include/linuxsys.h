// Linux calls the Darwin stand-in builds macOS functions on. Only headers that Swift's Glibc
// module lacks are included here (statfs, xattrs); the rest stay in linuxsys.c, so this module
// does not redefine Glibc's types.
#ifndef VPHONE_LINUX_SYS_H
#define VPHONE_LINUX_SYS_H
#include <sys/types.h>
#include <sys/vfs.h>
#include <sys/xattr.h>

int vl_ficlone(int dst, int src);
ssize_t vl_copy_file_range(int in, int out, size_t len);
int vl_punch_hole(int fd, off_t offset, off_t length);
int vl_renameat2_noreplace(const char *from, const char *to);
#endif
