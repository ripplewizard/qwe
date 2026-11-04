#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define SECRET_BYTES 32
#define TEST_ALLOCATION_BYTES (128U * 1024U * 1024U)

static SecKeychainRef keychain = NULL;
static char keychain_path[4096];
static char keychain_dir[4096];

static void cleanup_keychain(void) {
    if (keychain != NULL) {
        SecKeychainDelete(keychain);
        CFRelease(keychain);
        keychain = NULL;
    }
    if (keychain_path[0] != '\0') {
        unlink(keychain_path);
        char with_suffix[sizeof(keychain_path) + 16];
        snprintf(with_suffix, sizeof(with_suffix), "%s-db", keychain_path);
        unlink(with_suffix);
    }
    if (keychain_dir[0] != '\0') {
        rmdir(keychain_dir);
    }
}

static int keychain_probe(void) {
    unsigned char password[SECRET_BYTES];
    unsigned char secret[SECRET_BYTES];
    if (SecRandomCopyBytes(kSecRandomDefault, sizeof(password), password) != errSecSuccess ||
        SecRandomCopyBytes(kSecRandomDefault, sizeof(secret), secret) != errSecSuccess) {
        return 1;
    }

    char directory_template[] = "/tmp/qwe-stage0-keychain.XXXXXX";
    if (mkdtemp(directory_template) == NULL) {
        return 1;
    }
    strncpy(keychain_dir, directory_template, sizeof(keychain_dir) - 1);
    snprintf(
        keychain_path,
        sizeof(keychain_path),
        "%s/qwe-stage0.keychain",
        directory_template
    );
    if (atexit(cleanup_keychain) != 0) {
        return 1;
    }

    OSStatus create_status = SecKeychainCreate(
        keychain_path,
        sizeof(password),
        password,
        false,
        NULL,
        &keychain
    );
    if (create_status != errSecSuccess) {
        printf(
            "{\"keychain_create\":%d,\"keychain_add\":null,\"keychain_find\":null,"
            "\"keychain_read_matches\":false,\"keychain_delete_item\":null}\n",
            (int)create_status
        );
        return 1;
    }

    const char service[] = "com.qwe.stage0-probe";
    const char account[] = "ephemeral";
    SecKeychainItemRef item = NULL;
    OSStatus add_status = SecKeychainAddGenericPassword(
        keychain,
        (UInt32)strlen(service),
        service,
        (UInt32)strlen(account),
        account,
        sizeof(secret),
        secret,
        &item
    );
    if (add_status != errSecSuccess) {
        memset(password, 0, sizeof(password));
        memset(secret, 0, sizeof(secret));
        printf(
            "{\"keychain_create\":%d,\"keychain_add\":%d,\"keychain_find\":null,"
            "\"keychain_read_matches\":false,\"keychain_delete_item\":null}\n",
            (int)create_status,
            (int)add_status
        );
        return 1;
    }

    UInt32 found_length = 0;
    void *found_data = NULL;
    SecKeychainItemRef found_item = NULL;
    OSStatus find_status = SecKeychainFindGenericPassword(
        keychain,
        (UInt32)strlen(service),
        service,
        (UInt32)strlen(account),
        account,
        &found_length,
        &found_data,
        &found_item
    );
    int read_matches =
        find_status == errSecSuccess && found_length == sizeof(secret) &&
        memcmp(found_data, secret, sizeof(secret)) == 0;

    OSStatus delete_status = errSecSuccess;
    if (found_item != NULL) {
        delete_status = SecKeychainItemDelete(found_item);
    }
    if (found_data != NULL) {
        SecKeychainItemFreeContent(NULL, found_data);
    }
    if (found_item != NULL) {
        CFRelease(found_item);
    }
    if (item != NULL) {
        CFRelease(item);
    }

    memset(password, 0, sizeof(password));
    memset(secret, 0, sizeof(secret));
    printf(
        "{\"keychain_create\":%d,\"keychain_add\":%d,\"keychain_find\":%d,"
        "\"keychain_read_matches\":%s,\"keychain_delete_item\":%d}\n",
        (int)create_status,
        (int)add_status,
        (int)find_status,
        read_matches ? "true" : "false",
        (int)delete_status
    );
    return read_matches && delete_status == errSecSuccess ? 0 : 1;
}

static int child_exit_status(pid_t pid) {
    int status = 0;
    if (waitpid(pid, &status, 0) != pid) {
        return -1;
    }
    if (WIFEXITED(status)) {
        return WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status)) {
        return 128 + WTERMSIG(status);
    }
    return -1;
}

static int cpu_limit_probe(void) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        struct rlimit limit = {.rlim_cur = 1, .rlim_max = 1};
        if (setrlimit(RLIMIT_CPU, &limit) != 0) {
            _exit(30);
        }
        volatile uint64_t counter = 0;
        for (;;) {
            counter++;
        }
    }
    int status = child_exit_status(pid);
    if (status == 30) {
        return -2;
    }
    return status;
}

static int address_space_probe(void) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        struct rlimit limit = {
            .rlim_cur = 64U * 1024U * 1024U,
            .rlim_max = 64U * 1024U * 1024U,
        };
        if (setrlimit(RLIMIT_AS, &limit) != 0) {
            _exit(30);
        }
        void *allocation = mmap(
            NULL,
            TEST_ALLOCATION_BYTES,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0
        );
        if (allocation == MAP_FAILED) {
            _exit(20);
        }
        memset(allocation, 0x5a, TEST_ALLOCATION_BYTES);
        munmap(allocation, TEST_ALLOCATION_BYTES);
        _exit(21);
    }
    int status = child_exit_status(pid);
    if (status == 30) {
        return -2;
    }
    if (status == 20) {
        return 1;
    }
    if (status == 21) {
        return 0;
    }
    return -1;
}

static int process_count_probe(void) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        struct rlimit limit = {.rlim_cur = 1, .rlim_max = 1};
        if (setrlimit(RLIMIT_NPROC, &limit) != 0) {
            _exit(30);
        }
        pid_t child = fork();
        if (child < 0) {
            _exit(errno == EAGAIN ? 20 : 31);
        }
        if (child == 0) {
            _exit(0);
        }
        waitpid(child, NULL, 0);
        _exit(21);
    }
    int status = child_exit_status(pid);
    if (status == 30 || status == 31) {
        return -2;
    }
    if (status == 20) {
        return 1;
    }
    if (status == 21) {
        return 0;
    }
    return -1;
}

static const char *address_space_result(int status) {
    if (status == 1) {
        return "enforced";
    }
    if (status == 0) {
        return "not_enforced";
    }
    if (status == -2) {
        return "setrlimit_failed";
    }
    return "probe_failed";
}

static const char *process_count_result(int status) {
    if (status == 1) {
        return "enforced";
    }
    if (status == 0) {
        return "not_enforced";
    }
    if (status == -2) {
        return "setrlimit_failed";
    }
    return "probe_failed";
}

int main(void) {
    int keychain_status = keychain_probe();
    int cpu_status = cpu_limit_probe();
    int address_space_status = address_space_probe();
    int process_count_status = process_count_probe();

    printf(
        "{\"rlimit_cpu\":{\"exit_or_signal\":%d,\"setrlimit_failed\":%s},"
        "\"rlimit_address_space\":\"%s\",\"rlimit_process_count\":\"%s\"}\n",
        cpu_status,
        cpu_status == -2 ? "true" : "false",
        address_space_result(address_space_status),
        process_count_result(process_count_status)
    );

    if (keychain_status != 0 || cpu_status <= 1) {
        return 1;
    }
    return 0;
}
