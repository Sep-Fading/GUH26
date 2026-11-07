// Distinguishes contention from other system failures.
#include <errno.h>
// Acquires ownership on an inherited descriptor.
#include <sys/file.h>
// Validates the supplied regular file identity.
#include <sys/stat.h>
// Compares the lock owner with caller.
#include <unistd.h>

// Locks inherited descriptor 3, leaving the parent's shared reference intact.
int main(void) {
    struct stat metadata;
    if (fstat(3, &metadata) != 0 || !S_ISREG(metadata.st_mode)
        || metadata.st_uid != getuid() || metadata.st_nlink != 1
        || (metadata.st_mode & 077) != 0) return 2;
    if (flock(3, LOCK_EX | LOCK_NB) != 0) {
        return errno == EWOULDBLOCK ? 3 : 2;
    }
    // Explicit unlock would also release the parent's shared lock.
    return 0;
}
