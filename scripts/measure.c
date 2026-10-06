// SPDX-License-Identifier: MIT
// A small fresh monitor avoids charging the Python benchmark driver's RSS to forked scanners.
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 3) return 2;
    struct timespec begin, end;
    if (clock_gettime(CLOCK_MONOTONIC, &begin)) return 2;
    pid_t child = fork();
    if (child < 0) return 2;
    if (child == 0) { execvp(argv[2], argv + 2); perror("execvp"); _exit(127); }
    struct rusage usage = {0}; int status = 0;
    while (wait4(child, &status, 0, &usage) < 0) { if (errno != EINTR) return 2; }
    if (clock_gettime(CLOCK_MONOTONIC, &end)) return 2;
    int exit_status = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    FILE *out = fopen(argv[1], "w"); if (!out) return 2;
    double wall = end.tv_sec - begin.tv_sec + (end.tv_nsec - begin.tv_nsec) / 1e9;
    fprintf(out, "{\"wall_seconds\":%.9f,\"user_seconds\":%.6f,\"system_seconds\":%.6f,\"peak_rss_kib\":%ld,\"exit_status\":%d,\"minor_faults\":%ld,\"major_faults\":%ld,\"voluntary_switches\":%ld,\"involuntary_switches\":%ld}\n", wall, usage.ru_utime.tv_sec + usage.ru_utime.tv_usec / 1e6, usage.ru_stime.tv_sec + usage.ru_stime.tv_usec / 1e6, usage.ru_maxrss, exit_status, usage.ru_minflt, usage.ru_majflt, usage.ru_nvcsw, usage.ru_nivcsw);
    if (fclose(out)) return 2;
    return exit_status;
}
