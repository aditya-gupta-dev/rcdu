/* Fault injection for optional-statx fallback, confined to integration subprocesses. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
int statx(int dirfd, const char *name, int flags, unsigned int mask, struct statx *out) {
    (void)dirfd; (void)name; (void)flags; (void)mask;
    const char *failure = getenv("RCDU_TEST_STATX");
    if (failure && strcmp(failure, "partial-mask") == 0) {
        memset(out, 0, sizeof(*out));
        return 0;
    }
    errno = failure ? atoi(failure) : ENOSYS;
    return -1;
}
