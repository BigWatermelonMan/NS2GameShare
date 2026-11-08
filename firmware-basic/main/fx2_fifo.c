// FX2LP asynchronous slave FIFO, EP6 fixed selection, 512-byte AUTOIN.
// Four bytes use the i80 command DMA and 508 bytes use its color DMA.
// Together they form one exact USB packet; trailing padding occurs only
// after complete A5/5A wire frames, never in the middle of a framed record.
#include "fx2_fifo.h"
#include <string.h>
#include <stdlib.h>
#include <stdio.h>
#include "sdkconfig.h"
#include "driver/gpio.h"
#include "driver/uart.h"
#include "esp_lcd_io_i80.h"
#include "esp_lcd_panel_io.h"
#include "esp_timer.h"
#include "esp_rom_gpio.h"
#include "esp_rom_sys.h"
#include "esp_rom_uart.h"
#include "soc/gpio_reg.h"
#include "soc/gpio_sig_map.h"
#include "soc/i2s_struct.h"
#include "soc/soc.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
static esp_lcd_panel_io_handle_t s_io;
static SemaphoreHandle_t s_done;
static uint8_t s_block[512];
static uint32_t s_return_count;
static uint8_t s_first[8];
// Temporary bounded startup trace. Switch FD7 to UART0 only while SLWR is
// idle, wait for its last bit, then restore the i80 route before any DMA.
static void link_trace(const char *text)
{
#if CONFIG_BASIC_FX2_TRACE
    uint32_t route = REG_READ(GPIO_FUNC1_OUT_SEL_CFG_REG);
    REG_WRITE(GPIO_FUNC1_OUT_SEL_CFG_REG,
        (route & ~GPIO_FUNC1_OUT_SEL_M) | U0TXD_OUT_IDX);
    const char *prefix = "\r\nBASIC-TRACE ";
    while (*prefix) esp_rom_output_tx_one_char(*prefix++);
    while (*text) esp_rom_output_tx_one_char(*text++);
    esp_rom_output_tx_one_char('\r'); esp_rom_output_tx_one_char('\n');
    esp_rom_output_tx_wait_idle(0);
    REG_WRITE(GPIO_FUNC1_OUT_SEL_CFG_REG, route);
#else
    (void)text;
#endif
}
static bool transfer_done(esp_lcd_panel_io_handle_t io,
                          esp_lcd_panel_io_event_data_t *event, void *context)
{
    (void)io; (void)event;
    BaseType_t wake = pdFALSE;
    xSemaphoreGiveFromISR(context, &wake);
    return wake == pdTRUE;
}
esp_err_t fx2_fifo_init(void)
{
#if CONFIG_BASIC_FX2_TRACE
    uart_config_t trace_uart = {.baud_rate = 115200, .data_bits = UART_DATA_8_BITS,
        .parity = UART_PARITY_DISABLE, .stop_bits = UART_STOP_BITS_1,
        .flow_ctrl = UART_HW_FLOWCTRL_DISABLE, .source_clk = UART_SCLK_APB};
    ESP_ERROR_CHECK(uart_param_config(UART_NUM_0, &trace_uart));
    esp_rom_output_set_as_console(0);
#endif
    link_trace("init begin");
    const int pins[] = {CONFIG_BASIC_FX2_D0, CONFIG_BASIC_FX2_D1,
        CONFIG_BASIC_FX2_D2, CONFIG_BASIC_FX2_D3, CONFIG_BASIC_FX2_D4,
        CONFIG_BASIC_FX2_D5, CONFIG_BASIC_FX2_D6, CONFIG_BASIC_FX2_D7,
        CONFIG_BASIC_FX2_WR, CONFIG_BASIC_FX2_SPACE, CONFIG_BASIC_FX2_RX, 13};
    for (unsigned i = 0; i < sizeof(pins)/sizeof(pins[0]); ++i) {
        if (!GPIO_IS_VALID_GPIO(pins[i]) ||
            (i < 9 && !GPIO_IS_VALID_OUTPUT_GPIO(pins[i]))) return ESP_ERR_INVALID_ARG;
        for (unsigned j = 0; j < i; ++j)
            if (pins[i] == pins[j]) return ESP_ERR_INVALID_ARG;
        // Preserve flash, USB-UART TX into GPIO3, power I2C and onboard loads.
        // GPIO1 drives only the USB-UART's RX input and is available with
        // the application's UART0 transport disabled (ROM flashing still works).
        if ((pins[i] >= 6 && pins[i] <= 11) || pins[i] == 3 ||
            pins[i] == 4 || pins[i] == 14 || pins[i] == 21 || pins[i] == 22 ||
            pins[i] == 25 || pins[i] == 32) return ESP_ERR_INVALID_ARG;
#if CONFIG_SNIFFER_USE_UART
        if (pins[i] == 1) return ESP_ERR_INVALID_ARG;
#endif
    }
    // gpio_set_direction alone does not switch ADC/RTC/JTAG pin muxes.
    // gpio_config explicitly selects digital GPIO and releases RTC routing.
    gpio_set_level(14, 1); gpio_set_level(4, 1); // deselect LCD / SD
    gpio_set_level(32, 0); gpio_set_level(25, 0); // backlight / speaker off
    gpio_config_t outputs = {.pin_bit_mask = (1ULL<<14)|(1ULL<<4)|(1ULL<<32)|(1ULL<<25),
        .mode = GPIO_MODE_OUTPUT, .intr_type = GPIO_INTR_DISABLE};
    ESP_ERROR_CHECK(gpio_config(&outputs));
    gpio_config_t inputs = {.pin_bit_mask = (1ULL<<CONFIG_BASIC_FX2_SPACE)|(1ULL<<CONFIG_BASIC_FX2_RX),
        .mode = GPIO_MODE_INPUT, .intr_type = GPIO_INTR_DISABLE};
    ESP_ERROR_CHECK(gpio_config(&inputs));
    link_trace("digital GPIO configured");
    // GPIO35/36 have no internal pull resistors. External pulls are mandatory.
    s_done = xSemaphoreCreateBinary();
    if (!s_done) return ESP_ERR_NO_MEM;
    esp_lcd_i80_bus_handle_t bus;
    esp_lcd_i80_bus_config_t b = {
        .dc_gpio_num = 13, .wr_gpio_num = CONFIG_BASIC_FX2_WR,
        .clk_src = LCD_CLK_SRC_DEFAULT, .bus_width = 8, .max_transfer_bytes = 2048,
        .data_gpio_nums = {CONFIG_BASIC_FX2_D0, CONFIG_BASIC_FX2_D1,
            CONFIG_BASIC_FX2_D2, CONFIG_BASIC_FX2_D3, CONFIG_BASIC_FX2_D4,
            CONFIG_BASIC_FX2_D5, CONFIG_BASIC_FX2_D6, CONFIG_BASIC_FX2_D7},
    };
    ESP_ERROR_CHECK(esp_lcd_new_i80_bus(&b, &bus));
    link_trace("i80 bus ready");
    esp_lcd_panel_io_i80_config_t cfg = {
        .cs_gpio_num = -1, .pclk_hz = CONFIG_BASIC_FX2_CLOCK_HZ,
        .trans_queue_depth = 1, .lcd_cmd_bits = 32, .lcd_param_bits = 8,
        .on_color_trans_done = transfer_done, .user_ctx = s_done,
        .dc_levels = {.dc_idle_level = 0, .dc_cmd_level = 0, .dc_data_level = 0},
    };
    ESP_ERROR_CHECK(esp_lcd_new_panel_io_i80(bus, &cfg, &s_io));
    link_trace("i80 device ready");
    uart_config_t uart = {.baud_rate = 230400, .data_bits = UART_DATA_8_BITS,
        .parity = UART_PARITY_DISABLE, .stop_bits = UART_STOP_BITS_1,
        .flow_ctrl = UART_HW_FLOWCTRL_DISABLE, .source_clk = UART_SCLK_DEFAULT};
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_1, 8192, 0, 0, NULL, 0));
    ESP_ERROR_CHECK(uart_param_config(UART_NUM_1, &uart));
    return uart_set_pin(UART_NUM_1, UART_PIN_NO_CHANGE, CONFIG_BASIC_FX2_RX,
                         UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE);
}
esp_err_t fx2_fifo_write(const void *data, size_t length)
{
    const uint8_t *p = data;
    static unsigned trace_count;
    bool trace = trace_count++ < 4;
    if (trace) {
        char status[128]; fx2_link_diagnostics(status, sizeof(status)); link_trace(status);
    }
    while (length) {
        int64_t deadline = esp_timer_get_time() + 100000;
        // At packet boundaries EP6 not-full guarantees one complete bank.
        while (!gpio_get_level(CONFIG_BASIC_FX2_SPACE)) {
            if (esp_timer_get_time() >= deadline) return ESP_ERR_TIMEOUT;
            vTaskDelay(1);
        }
        size_t n = length < sizeof(s_block) ? length : sizeof(s_block);
        memset(s_block, 0, sizeof(s_block));
        memcpy(s_block, p, n);
        uint32_t command = ((uint32_t)s_block[0] << 24) | ((uint32_t)s_block[1] << 16) |
            ((uint32_t)s_block[2] << 8) | s_block[3];
        if (trace) link_trace("before i80 command/color");
        esp_err_t error = esp_lcd_panel_io_tx_color(s_io, (int)command, s_block + 4, 508);
        if (error != ESP_OK) return error;
        // Do not reuse the command/color buffer before DMA completes.
        if (xSemaphoreTake(s_done, pdMS_TO_TICKS(100)) != pdTRUE) abort();
        // ESP32's LCD bus is I2S0. DMA EOF alone can precede the last WR
        // edges: starting the next command immediately loses a byte per
        // block. Wait for the actual TX engine to become idle as well.
        int64_t idle_deadline = esp_timer_get_time() + 100000;
        while (!I2S0.state.tx_idle) {
            if (esp_timer_get_time() >= idle_deadline) abort();
        }
        if (trace) link_trace("DMA complete");
        p += n; length -= n;
    }
    return ESP_OK;
}
int fx2_return_read(uint8_t *data, size_t length)
{
    int count = uart_read_bytes(UART_NUM_1, data, length, pdMS_TO_TICKS(10));
    if (count > 0) {
        for (int i = 0; i < count && s_return_count + i < sizeof(s_first); ++i)
            s_first[s_return_count + i] = data[i];
        s_return_count += count;
    }
    return count;
}
void fx2_link_diagnostics(char *text, size_t capacity)
{
    snprintf(text, capacity, "basic link rx=%lu space=%d rxlevel=%d first=%02x%02x%02x%02x%02x%02x%02x%02x",
        (unsigned long)s_return_count, gpio_get_level(CONFIG_BASIC_FX2_SPACE),
        gpio_get_level(CONFIG_BASIC_FX2_RX), s_first[0],s_first[1],s_first[2],s_first[3],
        s_first[4],s_first[5],s_first[6],s_first[7]);
}
