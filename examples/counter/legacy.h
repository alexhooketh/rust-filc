#ifndef EXTERN_FILC_COUNTER_LEGACY_H
#define EXTERN_FILC_COUNTER_LEGACY_H

#include "extern_filc.h"

#include <stdint.h>

typedef struct counter counter_t;

int32_t legacy_add(int32_t left, int32_t right);
extern_filc_bytes legacy_reverse(extern_filc_bytes input);
void legacy_release_bytes(const uint8_t *bytes);
extern_filc_string legacy_greet(extern_filc_string name);
void legacy_release_string(const char *string);
counter_t *counter_new(int64_t initial);
int64_t counter_add(counter_t *counter, int64_t delta);
void counter_drop(counter_t *counter);
uint32_t legacy_trigger_oob(extern_filc_bytes input);

#endif
