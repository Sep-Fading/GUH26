// Reports bounded fixture metadata without paths.
#include <stdio.h>
// Compares only the fixed probe commands.
#include <string.h>
// Inspects inherited descriptor type and ownership.
#include <sys/stat.h>
// Requests the platform's stronger file flush.
#include <fcntl.h>
// Flushes directories and checks caller ownership.
#include <unistd.h>

// Inspects or flushes only an inherited private fixture descriptor.
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    struct stat metadata;
    if (fstat(3, &metadata) != 0 || metadata.st_uid != getuid()
        || (metadata.st_mode & 077) != 0) return 2;
    if (strcmp(argv[1], "sync-directory") == 0) {
        if (!S_ISDIR(metadata.st_mode)) return 2;
        return fsync(3) == 0 ? 0 : 2;
    }
    if (!S_ISREG(metadata.st_mode) || metadata.st_nlink != 1) return 2;
    if (strcmp(argv[1], "sync-file") == 0) {
        return fcntl(3, F_FULLFSYNC) == 0 ? 0 : 2;
    }
    if (strcmp(argv[1], "inspect") != 0) return 2;
    printf("{\"device\":\"%llu\",\"inode\":\"%llu\",\"generation\":\"%u\"}\n",
           (unsigned long long)metadata.st_dev, (unsigned long long)metadata.st_ino,
           metadata.st_gen);
    return 0;
}
