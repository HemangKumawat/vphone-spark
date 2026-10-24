/* vphone-linux: BSD funopen(3) for glibc, built on fopencookie(3). */
#ifdef __linux__
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>

struct funopen_cookie {
    void *cookie;
    int (*readfn)(void *, char *, int);
    int (*writefn)(void *, const char *, int);
    off_t (*seekfn)(void *, off_t, int);
    int (*closefn)(void *);
};

static ssize_t fc_read(void *c, char *buf, size_t size)
{
    struct funopen_cookie *f = c;
    return f->readfn ? f->readfn(f->cookie, buf, (int)size) : -1;
}

static ssize_t fc_write(void *c, const char *buf, size_t size)
{
    struct funopen_cookie *f = c;
    return f->writefn ? f->writefn(f->cookie, buf, (int)size) : -1;
}

static int fc_seek(void *c, off64_t *offset, int whence)
{
    struct funopen_cookie *f = c;
    if (!f->seekfn) return -1;
    off_t r = f->seekfn(f->cookie, (off_t)*offset, whence);
    if (r < 0) return -1;
    *offset = r;
    return 0;
}

static int fc_close(void *c)
{
    struct funopen_cookie *f = c;
    int r = f->closefn ? f->closefn(f->cookie) : 0;
    free(f);
    return r;
}

FILE *funopen(const void *cookie, int (*readfn)(void *, char *, int), int (*writefn)(void *, const char *, int),
              off_t (*seekfn)(void *, off_t, int), int (*closefn)(void *))
{
    struct funopen_cookie *f = malloc(sizeof(*f));
    if (!f) return NULL;
    f->cookie = (void *)cookie; f->readfn = readfn; f->writefn = writefn; f->seekfn = seekfn; f->closefn = closefn;
    cookie_io_functions_t io = { fc_read, fc_write, fc_seek, fc_close };
    const char *mode = readfn && writefn ? "r+" : (writefn ? "w" : "r");
    FILE *fp = fopencookie(f, mode, io);
    if (!fp) free(f);
    return fp;
}
#endif
