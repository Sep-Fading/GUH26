// Opens only the guest virtual socket.
#include <sys/socket.h>
// Addresses the hypervisor without IP networking.
#include <sys/vsock.h>
// Bounds stalled probes and ignores SIGPIPE.
#include <signal.h>
// Reports interrupted reads without losing state.
#include <errno.h>
// Reads handshake bytes and closes descriptors.
#include <unistd.h>
// Prints fixed diagnostics without guest data.
#include <stdio.h>

// Accepts one fixed greeting and returns success only after acknowledging it.
int master_probe(int fd) {
    char greeting = 0;
    ssize_t count;
    do { count = read(fd, &greeting, 1); } while (count < 0 && errno == EINTR);
    if (count != 1 || greeting != 'M') return 1;
    do { count = write(fd, "C", 1); } while (count < 0 && errno == EINTR);
    return count == 1 ? 0 : 1;
}

#ifndef MASTER_PROBE_TEST
// Tests guest transport without accepting commands, credentials, paths or destinations.
int main(void) {
    signal(SIGPIPE, SIG_IGN);
    alarm(5);
    int fd = socket(AF_VSOCK, SOCK_STREAM, 0);
    if (fd < 0) { perror("Guest virtual socket"); return 1; }
    struct sockaddr_vm address = {
        .svm_len = sizeof(address), .svm_family = AF_VSOCK,
        .svm_cid = VMADDR_CID_HOST, .svm_port = 10240,
    };
    int result = connect(fd, (struct sockaddr *)&address, sizeof(address));
    if (result < 0) perror("Guest virtual connection");
    else result = master_probe(fd);
    close(fd);
    alarm(0);
    if (result == 0) puts("Guest virtual socket handshake passed.");
    return result == 0 ? 0 : 1;
}
#endif
