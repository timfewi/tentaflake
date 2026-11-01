/* Preserve Semgrep's argv[0]-selected core/osemgrep dispatch without rewriting ELF. */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 1) {
        return 127;
    }
    char **args = calloc((size_t)argc + 6, sizeof(*args));
    if (args == NULL) {
        perror("semgrep loader allocation");
        return 127;
    }
    args[0] = LOADER;
    args[1] = "--library-path";
    args[2] = LIBRARIES;
    args[3] = "--argv0";
    args[4] = argv[0];
    args[5] = CORE;
    for (int i = 1; i < argc; ++i) {
        args[i + 5] = argv[i];
    }
    execv(LOADER, args);
    perror("semgrep dynamic loader");
    free(args);
    return 127;
}
