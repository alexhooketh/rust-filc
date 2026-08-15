#include "legacy.h"

#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

struct counter {
    int64_t value;
};

int32_t legacy_add(int32_t left, int32_t right) {
    return left + right;
}

extern_filc_bytes legacy_reverse(extern_filc_bytes input) {
    uint8_t *output = input.len == 0u ? NULL : malloc(input.len);
    if (output == NULL && input.len != 0u) {
        return (extern_filc_bytes){0};
    }
    for (size_t index = 0u; index < input.len; ++index) {
        output[index] = input.ptr[input.len - index - 1u];
    }
    return (extern_filc_bytes){.ptr = output, .len = input.len};
}

void legacy_release_bytes(const uint8_t *bytes) {
    free((void *)bytes);
}

extern_filc_string legacy_greet(extern_filc_string name) {
    static const char prefix[] = "hello, ";
    size_t prefix_len = sizeof(prefix) - 1u;
    if (name.len > SIZE_MAX - prefix_len) {
        return (extern_filc_string){0};
    }
    size_t length = prefix_len + name.len;
    char *output = length == 0u ? NULL : malloc(length);
    if (output == NULL && length != 0u) {
        return (extern_filc_string){0};
    }
    memcpy(output, prefix, prefix_len);
    if (name.len != 0u) {
        memcpy(output + prefix_len, name.ptr, name.len);
    }
    return (extern_filc_string){.ptr = output, .len = length};
}

void legacy_release_string(const char *string) {
    free((void *)string);
}

counter_t *counter_new(int64_t initial) {
    counter_t *counter = malloc(sizeof(*counter));
    if (counter != NULL) {
        counter->value = initial;
    }
    return counter;
}

int64_t counter_add(counter_t *counter, int64_t delta) {
    counter->value += delta;
    return counter->value;
}

void counter_drop(counter_t *counter) {
    free(counter);
}

uint32_t legacy_trigger_oob(extern_filc_bytes input) {
    volatile uint8_t *probe = malloc(1u);
    if (probe == NULL) {
        return 0u;
    }
    probe[0] = input.len == 0u ? 0u : input.ptr[0];
    uint32_t result = (uint32_t)probe[input.len + 1024u];
    return result;
}
