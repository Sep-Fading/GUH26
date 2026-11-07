// Opens only the supplied test lock.
#include <fcntl.h>
// Reports readiness to the parent harness.
#include <stdio.h>
// Holds an independent operating system lock.
#include <sys/file.h>
// Waits for stdin closure before release.
#include <unistd.h>

// Holds the fixture lock until its parent closes stdin.
int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int fd = open(argv[1], O_CREAT | O_RDWR | O_NOFOLLOW, 0600);
    if (fd < 0) return 2;
    if (flock(fd, LOCK_EX | LOCK_NB) != 0) {
        close(fd);
        return 3;
    }
    puts("locked");
    fflush(stdout);
    char byte;
    while (read(STDIN_FILENO, &byte, 1) > 0) {}
    close(fd);
    return 0;
}
