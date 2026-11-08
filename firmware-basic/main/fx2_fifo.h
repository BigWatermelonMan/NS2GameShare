#pragma once
#include <stddef.h>
#include <stdint.h>
#include "esp_err.h"
esp_err_t fx2_fifo_init(void);
esp_err_t fx2_fifo_write(const void *data, size_t length);
int fx2_return_read(uint8_t *data, size_t length);
void fx2_link_diagnostics(char *text, size_t capacity);
