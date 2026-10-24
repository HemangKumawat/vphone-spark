#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include "linuxsys.h"
#include <linux/fs.h>

int vl_ficlone(int dst, int src) { return ioctl(dst, FICLONE, src); }
ssize_t vl_copy_file_range(int in, int out, size_t len) { return copy_file_range(in, NULL, out, NULL, len, 0); }
int vl_punch_hole(int fd, off_t offset, off_t length)
{
    return fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, offset, length);
}
int vl_renameat2_noreplace(const char *from, const char *to)
{
    return renameat2(AT_FDCWD, from, AT_FDCWD, to, RENAME_NOREPLACE);
}
