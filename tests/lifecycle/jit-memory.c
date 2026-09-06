#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    long size = sysconf(_SC_PAGESIZE);
    if (size <= 0) return 2;
    void *page = mmap(0, (size_t)size, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) return 3;
    int result = mprotect(page, (size_t)size, PROT_READ | PROT_EXEC);
    munmap(page, (size_t)size);
    return result == 0 ? 0 : 77;
}
