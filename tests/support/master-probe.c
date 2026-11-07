// Asserts responses and child process results.
#include <assert.h>
// Opens disposable local socket pair fixtures.
#include <sys/socket.h>
// Waits for every owned test child.
#include <sys/wait.h>
// Sends bytes and closes fixture descriptors.
#include <unistd.h>
// Prints the independent handshake test result.
#include <stdio.h>

int master_probe(int fd);

// Runs one peer response and checks both halves without networking.
static void check(char greeting, int closed) {
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(pair[0]);
        if (!closed) {
            assert(write(pair[1], &greeting, 1) == 1);
            if (greeting == 'M') {
                char reply;
                assert(read(pair[1], &reply, 1) == 1 && reply == 'C');
            }
        }
        close(pair[1]);
        _exit(0);
    }
    close(pair[1]);
    assert(master_probe(pair[0]) == ((greeting == 'M' && !closed) ? 0 : 1));
    close(pair[0]);
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

// Exercises success and rejection without accessing an installed guest.
int main(void) {
    alarm(4);
    check('M', 0);
    check('X', 0);
    check('M', 1);
    puts("valid, wrong greeting, closed peer: passed");
    return 0;
}
