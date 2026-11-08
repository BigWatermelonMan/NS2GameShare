/*
 * ESP32-C5 Wi-Fi monitor-mode sniffer.
 *
 * Captured 802.11 frames are streamed to the PC over the native USB port
 * (USB-Serial-JTAG) or UART0, using a small framed binary protocol. The PC
 * side (host/c5sniff.py) turns the stream into pcap with radiotap headers for
 * Wireshark.
 *
 * Device -> host frame:
 *   A5 5A | type:u8 | len:u16le | payload[len] | crc16:u16le
 *   crc16 = CRC-16/CCITT-FALSE over type, len and payload.
 *   type 0x01 = captured packet (pkt_hdr_t + 802.11 frame without FCS)
 *   type 0x02 = text (command replies "ok ..."/"err ...", periodic "stats ...")
 *   type 0x04 = D2H_DATA (softsta payload: data_hdr_t + Ethernet frame)
 *   type 0x05 = EVENT  (structured text event from the softsta state machine)
 *
 * Host -> device: ASCII command lines terminated by '\n' (see handle_command)
 * and, since firmware 1.1, binary frames of the same framing with
 *   type 0x03 = H2D_DATA (softsta payload: data_hdr_t + Ethernet frame)
 * may appear on the stream at any time; a byte-level parser demultiplexes
 * them from command lines (see reader_task). Whichever port the last
 * command / data arrived on receives the output.
 */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"
#include "freertos/queue.h"
#include "freertos/ringbuf.h"

#include "esp_event.h"
#include "esp_mac.h"
#include "esp_system.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "esp_private/wifi.h"
#include "nvs_flash.h"
#include "sdkconfig.h"
#if CONFIG_IDF_TARGET_ESP32
#include "fx2_fifo.h"
#endif

#if CONFIG_SNIFFER_USE_UART
#include "driver/uart.h"
#include "soc/uart_pins.h"
#endif
#if CONFIG_SNIFFER_USE_USB_JTAG
#include "driver/usb_serial_jtag.h"
#endif

#if CONFIG_IDF_TARGET_ESP32
#define FW_VERSION "1.35-basic"
#else
#define FW_VERSION "1.28"
#endif

#define FRAME_TYPE_PKT      0x01
#define FRAME_TYPE_TEXT     0x02
#define FRAME_TYPE_H2D_DATA 0x03   /* host (app) -> device: data_hdr_t + Ethernet frame */
#define FRAME_TYPE_D2H_DATA 0x04   /* device -> host: data_hdr_t + Ethernet frame */
#define FRAME_TYPE_EVENT    0x05   /* device -> host: text status event */

#define MAX_CAPLEN   4000   /* hard cap per packet so ring items stay small */
#define MAX_H2D_LEN  4096   /* hard cap for one H2D_DATA payload */
#define MAX_HOP      64
#define MAX_MACS     8
#define FCS_LEN      4
#define STATS_PERIOD_US (5 * 1000 * 1000)

typedef struct __attribute__((packed)) {
    uint64_t ts_us;        /* esp_timer time at reception */
    uint16_t orig_len;     /* frame length on air, without FCS */
    uint8_t  channel;      /* primary channel the radio was on */
    uint8_t  second;       /* secondary channel info from rx_ctrl */
    int8_t   rssi;
    int8_t   noise_floor;
    uint8_t  bb_format;    /* wifi_rx_bb_format_t: 0=11b 1=11a/g 2=HT 3=VHT 4..7=HE */
    uint8_t  rate;         /* 11b: wifi_phy_rate_t, others: L-SIG rate field */
    uint32_t siga1;        /* HT-SIG / VHT-SIG-A / HE-SIG-A1 */
    uint16_t siga2;        /* HE-SIG-A2 */
    uint8_t  pkt_type;     /* wifi_promiscuous_pkt_type_t */
    uint8_t  reserved;
} pkt_hdr_t;

_Static_assert(sizeof(pkt_hdr_t) == 24, "pkt_hdr_t layout");

/* softsta data channel header (docs/softsta-design.md §4). */
typedef struct __attribute__((packed)) {
    uint64_t ts_us;        /* RX/TX time (esp_timer) */
    uint16_t frame_len;    /* Ethernet frame length (payload length - sizeof header) */
    uint8_t  channel;      /* channel at RX / unused at TX */
    uint8_t  rssi;         /* RSSI at RX / unused at TX */
    uint8_t  flags;        /* bit0: decrypted on device, bit1: broadcast,
                              bit2: broadcast TK (key id 1), bit3: retransmit */
    uint8_t  reserved;
    uint32_t seq;          /* app<->device sequence number (loss detection) */
} data_hdr_t;

/* 8+2+1+1+1+1+4 = 18. The design note that said 16 undercounted the header. */
_Static_assert(sizeof(data_hdr_t) == 18, "data_hdr_t layout");
#define DATA_HDR_SIZE ((uint16_t)sizeof(data_hdr_t))

typedef enum { PORT_NONE = 0, PORT_UART, PORT_USB, PORT_FX2 } port_t;

static RingbufHandle_t s_ring;
static SemaphoreHandle_t s_cfg_lock;
static TaskHandle_t s_hop_task;
static volatile port_t s_active_port = PORT_NONE;

/* Capture configuration. Written under s_cfg_lock, read lock-free by the RX callback. */
static volatile bool s_capturing;
static volatile uint16_t s_snaplen;          /* 0 = whole frame */
static volatile int8_t s_min_rssi = -128;
static uint8_t s_macs[MAX_MACS][6];
static volatile uint8_t s_mac_count;
static uint32_t s_filter_mask = WIFI_PROMIS_FILTER_MASK_MGMT | WIFI_PROMIS_FILTER_MASK_DATA;
static uint8_t s_hop_list[MAX_HOP];
static uint8_t s_hop_count;
static uint32_t s_dwell_ms = 200;
static volatile uint8_t s_cur_channel;

/* Statistics (only the Wi-Fi task writes the RX counters). */
static volatile uint32_t s_rx_total, s_rx_captured, s_rx_filtered, s_rx_dropped;

/* ------------------------------------------------------------------------- */
/* Softsta. Phase 0 is the serial protocol; phase 2 is open-system join
 * (probe / auth / assoc) copied from a real Switch 1 guest. CCMP is later. */
/* ------------------------------------------------------------------------- */

#define MODE_MONITOR 0
#define MODE_SOFTSTA 1

static volatile uint8_t s_mode;         /* MODE_* */
static volatile uint8_t s_h2d_echo;     /* sta loop: echo H2D frames back as D2H */
static volatile uint8_t s_sta_ready;    /* sta set succeeded */

static volatile uint32_t s_h2d_frames, s_d2h_frames, s_data_dropped;
static uint32_t s_rx_seq;               /* next sequence number for D2H frames */

typedef struct {
    uint8_t ssid[32];                   /* 16-byte session id (probe SSID is its hex) */
    uint8_t ssid_len;
    uint8_t bssid[6];
    uint8_t bssid_any;                  /* 1 = learn BSSID from the probe response */
    uint8_t channel;
    uint8_t tk[16];
    uint8_t tk_set;
} sta_cfg_t;

static sta_cfg_t s_sta;

#define STA_IDLE     0
#define STA_PROBING  1
#define STA_AUTH     2
#define STA_ASSOC    3
#define STA_LINKED   4

#define REQ_JOIN  1
#define REQ_LEAVE 2

#define MGMT_MAX 360

typedef struct {
    uint16_t len;
    uint8_t subtype;
    uint8_t data[MGMT_MAX];
} mgmt_item_t;

#define MGMT_SLOTS 6

static volatile uint8_t s_sta_state;
static volatile uint8_t s_sta_cancel;
static volatile uint8_t s_bssid_known;
static volatile uint8_t s_wake_hop;
static volatile int s_disc_reason = -1;
static volatile uint8_t s_native_up;
static uint8_t s_own_mac[6];
static uint8_t s_join_bssid[6];
static uint16_t s_tx_seq;
static QueueHandle_t s_sta_q;
static QueueHandle_t s_mgmt_q;          /* carries a pool index, not the frame */
static mgmt_item_t s_mgmt_pool[MGMT_SLOTS];
static volatile uint8_t s_mgmt_used[MGMT_SLOTS];

static void sta_on_rx(const uint8_t *frame, int len, wifi_promiscuous_pkt_type_t type);

/* ------------------------------------------------------------------------- */
/* Output                                                                    */
/* ------------------------------------------------------------------------- */

static uint16_t s_crc_table[256];

static void crc16_init(void)
{
    for (int i = 0; i < 256; i++) {
        uint16_t c = (uint16_t)(i << 8);
        for (int b = 0; b < 8; b++) {
            c = (c & 0x8000) ? (uint16_t)((c << 1) ^ 0x1021) : (uint16_t)(c << 1);
        }
        s_crc_table[i] = c;
    }
}

static uint16_t crc16_update(uint16_t crc, const uint8_t *p, size_t n)
{
    while (n--) {
        crc = (uint16_t)((crc << 8) ^ s_crc_table[((crc >> 8) ^ *p++) & 0xFF]);
    }
    return crc;
}

static void port_write(port_t port, const void *data, size_t len)
{
    switch (port) {
#if CONFIG_BASIC_FX2
    case PORT_FX2:
        if (fx2_fifo_write(data, len) != ESP_OK) s_data_dropped++;
        break;
#endif
#if CONFIG_SNIFFER_USE_UART
    case PORT_UART:
        uart_write_bytes(UART_NUM_0, data, len);
        break;
#endif
#if CONFIG_SNIFFER_USE_USB_JTAG
    case PORT_USB:
        /* A full TX ring may accept only a prefix. Preserve the remainder so
         * one timed-out write cannot cut a framed packet in the middle. */
        while (len && s_active_port == port) {
            int n = usb_serial_jtag_write_bytes(data, len, pdMS_TO_TICKS(50));
            if (n > 0) {
                data = (const uint8_t *)data + n;
                len -= (size_t)n;
            } else {
                vTaskDelay(1);
            }
        }
        break;
#endif
    default:
        break;
    }
}

/* Ring items are: type byte followed by the frame payload. One task drains the
 * ring so frames never interleave on the wire. Frames are packed into one batch
 * buffer and written with a single call: many small writes become short USB
 * packets, which caps the native USB link at roughly one transfer per millisecond. */
#define WRITE_BATCH 8192
#define INTERNAL_UART_BAUD 0xFE
static uint8_t s_batch[WRITE_BATCH];

static size_t frame_into(uint8_t *out, const uint8_t *item, size_t size)
{
    uint16_t len = (uint16_t)(size - 1);
    out[0] = 0xA5;
    out[1] = 0x5A;
    out[2] = item[0];
    out[3] = (uint8_t)len;
    out[4] = (uint8_t)(len >> 8);
    memcpy(out + 5, item + 1, len);
    uint16_t crc = crc16_update(0xFFFF, &out[2], 3 + len);
    out[5 + len] = (uint8_t)crc;
    out[6 + len] = (uint8_t)(crc >> 8);
    return (size_t)len + 7;
}

static void writer_task(void *arg)
{
    for (;;) {
        size_t size;
        uint8_t *item = xRingbufferReceive(s_ring, &size, portMAX_DELAY);
        size_t fill = 0;
        while (item) {
#if CONFIG_SNIFFER_USE_UART
            if (size == 5 && item[0] == INTERNAL_UART_BAUD) {
                /* Change rate at a frame boundary. The acknowledgement is sent
                 * at the old rate and drained before the UART divider changes. */
                if (fill) {
                    port_write(s_active_port, s_batch, fill);
                    fill = 0;
                }
                uint32_t baud;
                memcpy(&baud, item + 1, sizeof(baud));
                uint8_t reply[64];
                reply[0] = FRAME_TYPE_TEXT;
                int n = snprintf((char *)reply + 1, sizeof(reply) - 1,
                                 "ok baud %lu", (unsigned long)baud);
                size_t framed = frame_into(s_batch, reply, (size_t)n + 1);
                port_write(PORT_UART, s_batch, framed);
                ESP_ERROR_CHECK(uart_wait_tx_done(UART_NUM_0, pdMS_TO_TICKS(1000)));
                ESP_ERROR_CHECK(uart_set_baudrate(UART_NUM_0, baud));
                vRingbufferReturnItem(s_ring, item);
                item = xRingbufferReceive(s_ring, &size, 0);
                continue;
            }
#endif
            if (size >= 1 && size + 6 <= WRITE_BATCH) {
                if (fill + size + 6 > WRITE_BATCH) {
                    port_write(s_active_port, s_batch, fill);
                    fill = 0;
                }
                fill += frame_into(s_batch + fill, item, size);
            }
            vRingbufferReturnItem(s_ring, item);
            item = xRingbufferReceive(s_ring, &size, 0);
        }
        if (fill) {
            port_write(s_active_port, s_batch, fill);
        }
    }
}

/* Sends a framed data/event item through the ring (writer task frames it). */
static void send_pipe(uint8_t type, const void *payload, size_t len)
{
    uint8_t *dst;
    if (xRingbufferSendAcquire(s_ring, (void **)&dst, 1 + len, 0) != pdTRUE) {
        s_data_dropped++;
        return;
    }
    dst[0] = type;
    memcpy(dst + 1, payload, len);
    xRingbufferSendComplete(s_ring, dst);
}

static void send_text(const char *fmt, ...);
static void send_event(const char *fmt, ...);

/* Link throughput test: floods the ring with dummy packets (pkt_type 0xFF, which the
 * host drops) at a lower priority than the writer, so the count is what the link moved. */
#define BENCH_PKT_TYPE 0xFF
static void bench_task(void *arg)
{
    uint32_t ms = (uint32_t)(uintptr_t)arg;
    const size_t caplen = 1500;
    int64_t t0 = esp_timer_get_time();
    int64_t t_end = t0 + (int64_t)ms * 1000;
    uint32_t sent = 0;
    while (esp_timer_get_time() < t_end) {
        uint8_t *dst;
        if (xRingbufferSendAcquire(s_ring, (void **)&dst, 1 + sizeof(pkt_hdr_t) + caplen, 0) != pdTRUE) {
            vTaskDelay(1);
            continue;
        }
        dst[0] = FRAME_TYPE_PKT;
        pkt_hdr_t hdr = {.ts_us = (uint64_t)esp_timer_get_time(), .orig_len = caplen,
                         .pkt_type = BENCH_PKT_TYPE};
#if CONFIG_BASIC_FX2
        hdr.siga1 = sent;
#endif
        memcpy(dst + 1, &hdr, sizeof(hdr));
#if CONFIG_BASIC_FX2
        for (size_t i = 0; i < caplen; ++i) dst[1 + sizeof(hdr) + i] = (uint8_t)(i + sent);
#else
        memset(dst + 1 + sizeof(hdr), 0x55, caplen);
#endif
        xRingbufferSendComplete(s_ring, dst);
        sent++;
    }
    int64_t dt = esp_timer_get_time() - t0;
    vTaskDelay(pdMS_TO_TICKS(500));    /* let the writer drain the queue first */
    send_text("ok bench sent=%lu bytes=%lu us=%lld", (unsigned long)sent,
              (unsigned long)(sent * (1 + sizeof(pkt_hdr_t) + caplen + 7)), (long long)dt);
    vTaskDelete(NULL);
}

static void send_text(const char *fmt, ...)
{
    char buf[256];
    buf[0] = FRAME_TYPE_TEXT;
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf + 1, sizeof(buf) - 1, fmt, ap);
    va_end(ap);
    if (n < 0) {
        return;
    }
    if (n > (int)sizeof(buf) - 2) {
        n = sizeof(buf) - 2;
    }
    xRingbufferSend(s_ring, buf, (size_t)n + 1, pdMS_TO_TICKS(100));
}

/* Structured status events (softsta state machine, link status, overflow, ...).
 * Same text content as send_text but on the event channel, so an app can
 * consume events without mixing them into the command reply stream. */
static void send_event(const char *fmt, ...)
{
    char buf[256];
    buf[0] = FRAME_TYPE_EVENT;
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf + 1, sizeof(buf) - 1, fmt, ap);
    va_end(ap);
    if (n < 0) {
        return;
    }
    if (n > (int)sizeof(buf) - 2) {
        n = sizeof(buf) - 2;
    }
    xRingbufferSend(s_ring, buf, (size_t)n + 1, pdMS_TO_TICKS(100));
}

/* Non-blocking event for the Wi-Fi task (link drop). Drops the event if the
 * ring is full rather than stalling the receiver. */
static void send_event_nb(const char *msg)
{
    size_t n = strlen(msg);
    if (n > 180) {
        n = 180;
    }
    uint8_t buf[182];
    buf[0] = FRAME_TYPE_EVENT;
    memcpy(buf + 1, msg, n);
    xRingbufferSend(s_ring, buf, n + 1, 0);
}

/* ------------------------------------------------------------------------- */
/* Capture                                                                   */
/* ------------------------------------------------------------------------- */

static bool mac_matches(const uint8_t *frame, int len)
{
    uint8_t count = s_mac_count;
    if (count == 0) {
        return true;
    }
    /* addr1 @4, addr2 @10, addr3 @16 (control frames may only carry addr1/addr2) */
    static const int offs[3] = {4, 10, 16};
    for (int a = 0; a < 3; a++) {
        if (len < offs[a] + 6) {
            break;
        }
        for (int m = 0; m < count; m++) {
            if (memcmp(frame + offs[a], s_macs[m], 6) == 0) {
                return true;
            }
        }
    }
    return false;
}

static int mac_is_bcast(const uint8_t *m)
{
    return (m[0] & m[1] & m[2] & m[3] & m[4] & m[5]) == 0xFF;
}

/* Queue management frames addressed to us while a join/link is active.
 * Runs in the Wi-Fi task: no blocking, no logging. */
static void sta_on_rx(const uint8_t *frame, int len, wifi_promiscuous_pkt_type_t type)
{
    uint8_t st = s_sta_state;
    if (st == STA_IDLE || type != WIFI_PKT_MGMT || len < 24 || s_mgmt_q == NULL) {
        return;
    }
    __sync_synchronize();
    uint16_t fc = (uint16_t)(frame[0] | (frame[1] << 8));
    uint8_t subtype = (uint8_t)((fc >> 4) & 0xF);
    int to_us = memcmp(frame + 4, s_own_mac, 6) == 0;
    int bcast = mac_is_bcast(frame + 4);
    if (!to_us && !bcast) {
        return;
    }
    if (bcast) {
        if ((subtype != 10 && subtype != 12) || !s_bssid_known ||
                memcmp(frame + 16, s_join_bssid, 6) != 0) {
            return;
        }
    }
    if (st == STA_LINKED && (subtype == 10 || subtype == 12) && len >= 26) {
        uint16_t reason = (uint16_t)(frame[24] | (frame[25] << 8));
        s_sta_state = STA_IDLE;
        s_bssid_known = 0;
        char msg[64];
        snprintf(msg, sizeof(msg), "sta link dropped reason=%u", reason);
        send_event_nb(msg);
        return;
    }
    int idx = -1;
    for (int i = 0; i < MGMT_SLOTS; i++) {
        if (!s_mgmt_used[i]) {
            s_mgmt_used[i] = 1;
            idx = i;
            break;
        }
    }
    if (idx < 0) {
        return;
    }
    mgmt_item_t *item = &s_mgmt_pool[idx];
    item->len = (uint16_t)(len > MGMT_MAX ? MGMT_MAX : len);
    item->subtype = subtype;
    memcpy(item->data, frame, item->len);
    uint8_t slot = (uint8_t)idx;
    if (xQueueSend(s_mgmt_q, &slot, 0) != pdTRUE) {
        s_mgmt_used[idx] = 0;
    }
}

static void promisc_cb(void *buf, wifi_promiscuous_pkt_type_t type)
{
    const wifi_promiscuous_pkt_t *pkt = buf;
#if CONFIG_IDF_TARGET_ESP32
    const wifi_pkt_rx_ctrl_t *rx = &pkt->rx_ctrl;
#else
    const esp_wifi_rxctrl_t *rx = &pkt->rx_ctrl;
#endif

    if (type == WIFI_PKT_MISC || rx->rx_state != 0) {
        return;
    }
    int len = (int)rx->sig_len - FCS_LEN;
    if (len > 0) {
        sta_on_rx(pkt->payload, len, type);
    }
    if (!s_capturing || len <= 0) {
        return;
    }
    s_rx_total++;
    if (rx->rssi < s_min_rssi || !mac_matches(pkt->payload, len)) {
        s_rx_filtered++;
        return;
    }

    /* Snap length only trims data frames: management frames are rare and wanted whole. */
    int cap = len;
    uint16_t snap = s_snaplen;
    if (snap && type == WIFI_PKT_DATA && cap > snap) {
        cap = snap;
    }
    if (cap > MAX_CAPLEN) {
        cap = MAX_CAPLEN;
    }

    uint8_t *dst;
    if (xRingbufferSendAcquire(s_ring, (void **)&dst, 1 + sizeof(pkt_hdr_t) + cap, 0) != pdTRUE) {
        s_rx_dropped++;
        return;
    }
    dst[0] = FRAME_TYPE_PKT;
    pkt_hdr_t hdr = {
        .ts_us = (uint64_t)esp_timer_get_time(),
        .orig_len = (uint16_t)len,
        .channel = s_cur_channel,
#if CONFIG_IDF_TARGET_ESP32
        .second = (uint8_t)rx->secondary_channel,
#else
        .second = (uint8_t)rx->second,
#endif
        .rssi = (int8_t)rx->rssi,
        .noise_floor = (int8_t)rx->noise_floor,
#if CONFIG_IDF_TARGET_ESP32
        .bb_format = rx->sig_mode == 1 ? 2 : 0,
        .siga1 = 0,
        .siga2 = 0,
#else
        .bb_format = (uint8_t)rx->cur_bb_format,
        .siga1 = rx->he_siga1,
        .siga2 = rx->he_siga2,
#endif
        .rate = (uint8_t)rx->rate,
        .pkt_type = (uint8_t)type,
    };
    memcpy(dst + 1, &hdr, sizeof(hdr));
    memcpy(dst + 1 + sizeof(hdr), pkt->payload, cap);
    xRingbufferSendComplete(s_ring, dst);
    s_rx_captured++;
}

static bool channel_valid(int ch)
{
    if (ch >= 1 && ch <= 14) {
        return true;
    }
    static const uint8_t ch5g[] = {36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116,
                                   120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161, 165,
                                   169, 173, 177};
    for (size_t i = 0; i < sizeof(ch5g); i++) {
        if (ch5g[i] == ch) {
            return true;
        }
    }
    return false;
}

static esp_err_t tune(uint8_t ch)
{
    esp_err_t err = esp_wifi_set_channel(ch, WIFI_SECOND_CHAN_NONE);
    if (err == ESP_OK) {
        s_cur_channel = ch;
    }
    return err;
}

static void hop_task(void *arg)
{
    uint8_t idx = 0;
    for (;;) {
        uint32_t wait_ms = 1000;
        xSemaphoreTake(s_cfg_lock, portMAX_DELAY);
        if (s_hop_count > 1) {
            idx = (uint8_t)((idx + 1) % s_hop_count);
            tune(s_hop_list[idx]);
            wait_ms = s_dwell_ms;
        } else {
            idx = 0;
        }
        xSemaphoreGive(s_cfg_lock);
        /* A config change notifies us so a new hop list takes effect at once. */
        if (ulTaskNotifyTake(pdTRUE, pdMS_TO_TICKS(wait_ms))) {
            idx = (uint8_t)(s_hop_count ? s_hop_count - 1 : 0);
        }
    }
}

static void apply_filter(void)
{
    wifi_promiscuous_filter_t f = {.filter_mask = s_filter_mask};
    esp_wifi_set_promiscuous_filter(&f);
    if (s_filter_mask & WIFI_PROMIS_FILTER_MASK_CTRL) {
        wifi_promiscuous_filter_t cf = {.filter_mask = WIFI_PROMIS_CTRL_FILTER_MASK_ALL};
        esp_wifi_set_promiscuous_ctrl_filter(&cf);
    }
}

static void stats_timer_cb(void *arg)
{
#if CONFIG_BASIC_FX2
    if (!s_capturing) {
        char text[128];
        fx2_link_diagnostics(text, sizeof(text));
        send_event("%s", text);
    }
#endif
    if (s_capturing) {
        send_text("stats rx=%lu cap=%lu filt=%lu drop=%lu ch=%u",
                  (unsigned long)s_rx_total, (unsigned long)s_rx_captured,
                  (unsigned long)s_rx_filtered, (unsigned long)s_rx_dropped, s_cur_channel);
    }
}

/* ------------------------------------------------------------------------- */
/* Commands                                                                  */
/* ------------------------------------------------------------------------- */

static bool parse_mac(const char *s, uint8_t out[6])
{
    unsigned v[6];
    if (sscanf(s, "%x:%x:%x:%x:%x:%x", &v[0], &v[1], &v[2], &v[3], &v[4], &v[5]) != 6) {
        return false;
    }
    for (int i = 0; i < 6; i++) {
        if (v[i] > 0xFF) {
            return false;
        }
        out[i] = (uint8_t)v[i];
    }
    return true;
}

static int hexval(char c)
{
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
        return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
        return c - 'A' + 10;
    }
    return -1;
}

/* Parses a hex string ("ab01.." or "ab:01..") into out; returns number of bytes. */
static size_t parse_hex(const char *s, uint8_t *out, size_t max)
{
    size_t n = 0;
    int hi = -1;
    for (; *s && n < max; s++) {
        int v = hexval(*s);
        if (v < 0) {
            continue;        /* ignore separators */
        }
        if (hi < 0) {
            hi = v;
        } else {
            out[n++] = (uint8_t)((hi << 4) | v);
            hi = -1;
        }
    }
    return (hi < 0) ? n : 0; /* trailing half byte -> error */
}

/* Ethernet frame in, 802.11 (CCMP, once a key is installed) out. */
extern int esp_wifi_internal_tx(wifi_interface_t ifx, void *buffer, uint16_t len);
static void refresh_ldn_key(void);

/* The station driver supplies Ethernet after its RX/CCMP processing. There
 * is no esp_netif in this firmware: this is the sole owner of the RX buffer.
 * Copy into the serial ring without blocking the Wi-Fi task, then ALWAYS
 * release eb (including monitor mode, invalid frames and a full ring). */
static esp_err_t sta_ethernet_rx(void *buffer, uint16_t len, void *eb)
{
    if (s_mode == MODE_SOFTSTA && s_native_up && !s_sta_cancel) {
        uint32_t seq = s_rx_seq++;
        uint8_t *dst;
        if (buffer == NULL || len < 14 || len > MAX_H2D_LEN - DATA_HDR_SIZE ||
                xRingbufferSendAcquire(s_ring, (void **)&dst,
                                       1 + DATA_HDR_SIZE + len, 0) != pdTRUE) {
            s_data_dropped++;
        } else {
            /* This callback has no RSSI, key-id or original Protected bit.
             * Do not claim CCMP decryption/retry metadata we cannot observe. */
            data_hdr_t hdr = {
                .ts_us = (uint64_t)esp_timer_get_time(), .frame_len = len,
                .channel = s_cur_channel,
                .flags = mac_is_bcast(buffer) ? 0x02 : 0,
                .seq = seq,
            };
            dst[0] = FRAME_TYPE_D2H_DATA;
            memcpy(dst + 1, &hdr, sizeof(hdr));
            memcpy(dst + 1 + sizeof(hdr), buffer, len);
            xRingbufferSendComplete(s_ring, dst);
            s_d2h_frames++;
        }
    }
    if (eb != NULL) {
        esp_wifi_internal_free_rx_buffer(eb);
    }
    return ESP_OK;
}

/* H2D: data_hdr_t + Ethernet frame. Echo mode mirrors it. While linked, the
 * Ethernet frame is handed to the station stack, which encrypts it. */
static void handle_data_frame(const uint8_t *payload, uint16_t len)
{
    s_h2d_frames++;
    if (s_h2d_echo && len >= DATA_HDR_SIZE) {
        send_pipe(FRAME_TYPE_D2H_DATA, payload, len);
        s_d2h_frames++;
        return;
    }
    if (len < DATA_HDR_SIZE + 14 || s_sta_state != STA_LINKED) {
        send_event("sta tx dropped len=%u linked=%u", len, s_sta_state == STA_LINKED);
        return;
    }
    uint16_t elen = (uint16_t)(len - DATA_HDR_SIZE);
    const uint8_t *body = payload + DATA_HDR_SIZE;
    uint16_t declared_len = (uint16_t)(payload[8] | (payload[9] << 8));
    if (declared_len != elen) {
        s_data_dropped++;
        send_event("sta tx dropped length mismatch declared=%u actual=%u", declared_len, elen);
        return;
    }
    /* flags at data_hdr offset 12. Bit 4: body is a finished 802.11 frame. */
    if (payload[12] & 0x10) {
        /* After association, en_sys_seq must be true or the driver returns
         * ESP_ERR_INVALID_ARG. It rewrites only the sequence number. */
        esp_err_t e = esp_wifi_80211_tx(WIFI_IF_STA, body, elen, true);
        send_event("sta tx dot11 %s len=%u", esp_err_to_name(e), elen);
        return;
    }
    refresh_ldn_key();
    int rc = esp_wifi_internal_tx(WIFI_IF_STA, (void *)body, elen);
    if (rc != 0) {
        send_event("sta tx data rc=%d len=%u", rc, elen);
    } else {
        send_event("sta tx data len=%u", elen);
    }
}

static bool parse_channel_list(char *s, char *err, size_t errlen)
{
    uint8_t list[MAX_HOP];
    uint8_t n = 0;
    for (char *tok = strtok(s, ","); tok; tok = strtok(NULL, ",")) {
        int ch = atoi(tok);
        if (!channel_valid(ch)) {
            snprintf(err, errlen, "bad channel %s", tok);
            return false;
        }
        if (n == MAX_HOP) {
            snprintf(err, errlen, "too many channels (max %d)", MAX_HOP);
            return false;
        }
        list[n++] = (uint8_t)ch;
    }
    if (n == 0) {
        snprintf(err, errlen, "empty channel list");
        return false;
    }
    memcpy(s_hop_list, list, n);
    s_hop_count = n;
    return true;
}

/* ------------------------------------------------------------------------- */
/* Soft STA join (phase 2)                                                   */
/*                                                                           */
/* Management bodies below are the fixed IEs from a Switch 1 guest that      */
/* associated to a Switch 2 GameShare host (captures/2026-11-02_gameshare_   */
/* join_ch6_raw.pcap). SSID, channel, addresses and sequence are filled in.  */
/* ------------------------------------------------------------------------- */

/* Probe Request, after the 32-byte ASCII SSID. DS channel is byte 18. */
static const uint8_t PROBE_AFTER_SSID[] = {
    0x01, 0x04, 0x02, 0x04, 0x0b, 0x16,
    0x32, 0x08, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
    0x03, 0x01, 0x06,
    0x2d, 0x1a, 0x20, 0x00, 0x17, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x7f, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
    0xdd, 0x09, 0x00, 0x10, 0x18, 0x02, 0x00, 0x00, 0x10, 0x00, 0x00,
};
#define PROBE_DS_CHANNEL_OFF 18

/* Association Request, after the SSID IE. Supported-channels first channel
 * is byte 22 (tag 0x24). */
static const uint8_t ASSOC_AFTER_SSID[] = {
    0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x24, 0x30, 0x48, 0x6c,
    0x32, 0x04, 0x0c, 0x12, 0x18, 0x60,
    0x21, 0x02, 0x03, 0x13,
    0x24, 0x02, 0x01, 0x0d,
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04,
    0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
    0x2d, 0x1a, 0x20, 0x00, 0x17, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x7f, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
    0xdd, 0x06, 0x00, 0x22, 0xaa, 0x10, 0x01, 0x01,
    0xdd, 0x09, 0x00, 0x10, 0x18, 0x02, 0x00, 0x00, 0x10, 0x00, 0x00,
    0xdd, 0x07, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00,
};
#define ASSOC_SUPP_CH_OFF 22

static const uint8_t AUTH_BODY[] = {
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0xdd, 0x09, 0x00, 0x10, 0x18, 0x02, 0x00, 0x00, 0x10, 0x00, 0x00,
};
static const uint8_t BCAST_MAC[6] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};

_Static_assert(sizeof(PROBE_AFTER_SSID) == 68, "probe IE tail");
_Static_assert(24 + 2 + 32 + sizeof(PROBE_AFTER_SSID) == 126, "probe length");
_Static_assert(sizeof(ASSOC_AFTER_SSID) == 112, "assoc IE tail");
_Static_assert(24 + 4 + 2 + 32 + sizeof(ASSOC_AFTER_SSID) == 174, "assoc length");
_Static_assert(sizeof(AUTH_BODY) == 17, "auth body");

static void mac_fmt(const uint8_t m[6], char out[18])
{
    snprintf(out, 18, "%02x:%02x:%02x:%02x:%02x:%02x",
             m[0], m[1], m[2], m[3], m[4], m[5]);
}

static const char *sta_state_name(uint8_t st)
{
    switch (st) {
    case STA_PROBING: return "probing";
    case STA_AUTH:    return "auth";
    case STA_ASSOC:   return "assoc";
    case STA_LINKED:  return "linked";
    default:          return "idle";
    }
}

static void write_mgmt_hdr(uint8_t *f, uint8_t subtype,
                           const uint8_t *da, const uint8_t *sa, const uint8_t *bssid)
{
    uint16_t seq = (uint16_t)(s_tx_seq & 0x0FFF);
    s_tx_seq = (uint16_t)((s_tx_seq + 1) & 0x0FFF);
    uint16_t sc = (uint16_t)(seq << 4);
    f[0] = (uint8_t)(subtype << 4);
    f[1] = 0;
    f[2] = 0;
    f[3] = 0;
    memcpy(f + 4, da, 6);
    memcpy(f + 10, sa, 6);
    memcpy(f + 16, bssid, 6);
    f[22] = (uint8_t)sc;
    f[23] = (uint8_t)(sc >> 8);
}

static void format_ssid_ascii(const uint8_t bin[16], uint8_t ascii[32])
{
    static const char hexd[] = "0123456789abcdef";
    for (int i = 0; i < 16; i++) {
        ascii[i * 2] = (uint8_t)hexd[bin[i] >> 4];
        ascii[i * 2 + 1] = (uint8_t)hexd[bin[i] & 0x0F];
    }
}

static bool tx_mgmt(const uint8_t *frame, int len, const char *name)
{
    esp_err_t e = esp_wifi_80211_tx(WIFI_IF_STA, frame, len, false);
    if (e != ESP_OK) {
        send_event("sta tx %s failed %s", name, esp_err_to_name(e));
        return false;
    }
    return true;
}

static int build_probe(uint8_t *f, const uint8_t sa[6], const uint8_t ascii_ssid[32], uint8_t channel)
{
    write_mgmt_hdr(f, 4, BCAST_MAC, sa, BCAST_MAC);
    f[24] = 0;
    f[25] = 32;
    memcpy(f + 26, ascii_ssid, 32);
    memcpy(f + 58, PROBE_AFTER_SSID, sizeof(PROBE_AFTER_SSID));
    f[58 + PROBE_DS_CHANNEL_OFF] = channel;
    return 58 + (int)sizeof(PROBE_AFTER_SSID);
}

static int __attribute__((unused)) build_auth(uint8_t *f, const uint8_t sa[6], const uint8_t bssid[6])
{
    write_mgmt_hdr(f, 11, bssid, sa, bssid);
    memcpy(f + 24, AUTH_BODY, sizeof(AUTH_BODY));
    return 24 + (int)sizeof(AUTH_BODY);
}

static int __attribute__((unused)) build_assoc(uint8_t *f, const uint8_t sa[6], const uint8_t bssid[6],
                       const uint8_t ascii_ssid[32], uint8_t channel)
{
    write_mgmt_hdr(f, 0, bssid, sa, bssid);
    f[24] = 0x31;               /* ESS | privacy | short preamble */
    f[25] = 0x04;               /* short slot */
    f[26] = 10;                 /* listen interval */
    f[27] = 0;
    f[28] = 0;
    f[29] = 32;
    memcpy(f + 30, ascii_ssid, 32);
    memcpy(f + 62, ASSOC_AFTER_SSID, sizeof(ASSOC_AFTER_SSID));
    if (channel > 14) {
        f[62 + ASSOC_SUPP_CH_OFF] = channel;
        f[62 + ASSOC_SUPP_CH_OFF + 1] = 1;
    }
    return 62 + (int)sizeof(ASSOC_AFTER_SSID);
}

static int build_disassoc(uint8_t *f, const uint8_t sa[6], const uint8_t bssid[6])
{
    write_mgmt_hdr(f, 10, bssid, sa, bssid);
    f[24] = 8;                  /* STA leaving */
    f[25] = 0;
    return 26;
}

static bool ie_has_ssid(const uint8_t *frame, int len, int ie_off, const uint8_t ascii[32])
{
    int p = ie_off;
    while (p + 2 <= len) {
        int id = frame[p];
        int l = frame[p + 1];
        if (p + 2 + l > len) {
            return false;
        }
        if (id == 0) {
            return l == 32 && memcmp(frame + p + 2, ascii, 32) == 0;
        }
        p += 2 + l;
    }
    return false;
}

static void flush_mgmt(void)
{
    uint8_t idx;
    if (s_mgmt_q == NULL) {
        return;
    }
    while (xQueueReceive(s_mgmt_q, &idx, 0) == pdTRUE) {
        if (idx < MGMT_SLOTS) {
            s_mgmt_used[idx] = 0;
        }
    }
}

static bool wait_mgmt(uint8_t subtype, int timeout_ms, mgmt_item_t *out)
{
    int64_t deadline = esp_timer_get_time() + (int64_t)timeout_ms * 1000;
    while (esp_timer_get_time() < deadline) {
        if (s_sta_cancel) {
            return false;
        }
        int64_t left_us = deadline - esp_timer_get_time();
        TickType_t ticks = pdMS_TO_TICKS((left_us + 999) / 1000);
        if (ticks < 1) {
            ticks = 1;
        }
        uint8_t idx;
        if (xQueueReceive(s_mgmt_q, &idx, ticks) != pdTRUE) {
            continue;
        }
        if (idx >= MGMT_SLOTS) {
            continue;
        }
        mgmt_item_t *item = &s_mgmt_pool[idx];
        bool match = item->subtype == subtype;
        if (match) {
            *out = *item;
        }
        s_mgmt_used[idx] = 0;
        if (match) {
            return true;
        }
    }
    return false;
}

static void remember_bssid(const uint8_t bssid[6])
{
    memcpy(s_join_bssid, bssid, 6);
    __sync_synchronize();
    s_bssid_known = 1;
    xSemaphoreTake(s_cfg_lock, portMAX_DELAY);
    memcpy(s_sta.bssid, bssid, 6);
    s_sta.bssid_any = 0;
    xSemaphoreGive(s_cfg_lock);
}

static bool from_bssid(const uint8_t *frame, const uint8_t bssid[6])
{
    return memcmp(frame + 10, bssid, 6) == 0 || memcmp(frame + 16, bssid, 6) == 0;
}

static void tx_disassoc(const uint8_t sa[6], const uint8_t bssid[6])
{
    uint8_t frame[32];
    int n = build_disassoc(frame, sa, bssid);
    tx_mgmt(frame, n, "disassoc");
}

/* The Wi-Fi library does not export these. They are what the supplicant calls
 * to install a CCMP key and to tell the driver the 4-way handshake is done. */
#define WIFI_WPA_ALG_CCMP 3
#define KEY_FLAG_RX       (1 << 2)
#define KEY_FLAG_TX       (1 << 3)
#define KEY_FLAG_GROUP    (1 << 4)
#define KEY_FLAG_PAIRWISE (1 << 5)
extern int esp_wifi_set_sta_key_internal(int alg, uint8_t *addr, int key_idx, int set_tx,
                                         uint8_t *seq, size_t seq_len, uint8_t *key, size_t key_len,
                                         int key_flag);
extern void esp_wifi_auth_done_internal(void);
extern uint8_t esp_wifi_sta_get_pairwise_cipher_internal(void);
extern uint8_t esp_wifi_sta_get_group_cipher_internal(void);
extern int esp_wifi_get_sta_key_internal(uint8_t *ifx, int *alg, uint8_t *addr, int *key_idx,
                                         uint8_t *key, size_t key_len, int key_flag);

/* Compares one installed key with the LDN TK. The key itself is never logged. */
static void read_key_match(int flag, int idx, const uint8_t tk[16], int *rc_out, int *match_out)
{
    uint8_t ifx = 0;
    int alg = -1;
    int key_idx = idx;
    uint8_t addr[6] = {0};
    uint8_t key[32];
    memset(key, 0, sizeof(key));
    int rc = esp_wifi_get_sta_key_internal(&ifx, &alg, addr, &key_idx, key, 16, flag);
    *rc_out = rc;
    *match_out = rc == 0 && memcmp(key, tk, 16) == 0;
    memset(key, 0, sizeof(key));
}

static void note_installed_key(const char *when, const uint8_t tk[16])
{
    int grc, gmatch;
    /* IDF 5.5.6's getter only implements group-key reads. A -1 for a
     * pairwise read is not evidence that the pairwise TK was overwritten. */
    read_key_match(KEY_FLAG_GROUP, 1, tk, &grc, &gmatch);
    send_event("sta key %s pair=unreadable group rc=%d match=%d", when, grc, gmatch);
}

static int s_key_logged;
static int install_ldn_keys(uint8_t bssid[6], const uint8_t tk[16], bool open_port);

/* The supplicant can replace the TK with a PSK-derived PTK after auth_done.
 * Put the LDN key back immediately before a data frame leaves. */
static void refresh_ldn_key(void)
{
    static int64_t next_check;
    int64_t now = esp_timer_get_time();
    if (s_key_logged && now < next_check) {
        return;
    }
    next_check = now + 1000000;
    sta_cfg_t cfg;
    xSemaphoreTake(s_cfg_lock, portMAX_DELAY);
    cfg = s_sta;
    xSemaphoreGive(s_cfg_lock);
    if (!cfg.tk_set) {
        return;
    }
    if (!s_key_logged) {
        note_installed_key("before", cfg.tk);
    }
    int rc, match;
    read_key_match(KEY_FLAG_GROUP, 1, cfg.tk, &rc, &match);
    /* Do not reinstall both keys on every input packet. The TX cipher hook
     * already repairs the pairwise TK; reinstall only initially or when the
     * readable group key was actually replaced by the supplicant. */
    if (!s_key_logged || rc != 0 || !match) {
        if (install_ldn_keys(s_join_bssid, cfg.tk, false) == 0) {
            if (!s_key_logged) {
                note_installed_key("after", cfg.tk);
            }
            s_key_logged = 1;
        }
    }
    memset(&cfg, 0, sizeof(cfg));
}

/* Runs in the Wi-Fi task. data starts at the MAC header; data_len is the body. */
#if !CONFIG_IDF_TARGET_ESP32
static void on_80211_tx(const wifi_tx_info_t *info)
{
    if (info == NULL || info->data == NULL || info->data_len < 4) {
        return;
    }
    uint8_t fc0 = info->data[0];
    if ((fc0 & 0x0C) != 0x08) {
        return;
    }
    int hdr = (fc0 & 0x80) ? 26 : 24;
    static uint8_t logged;
    if (info->data_len < hdr + 4 || logged >= 16) {
        return;
    }
    logged++;
    const uint8_t *body = info->data + hdr;
    char msg[96];
    snprintf(msg, sizeof(msg),
             "sta air fc=%02x%02x prot=%u body=%u b=%02x%02x%02x%02x st=%u",
             fc0, info->data[1], (unsigned)((info->data[1] >> 6) & 1),
             info->data_len, body[0], body[1], body[2], body[3],
             (unsigned)info->tx_status);
    send_event_nb(msg);
}
#endif

/* libnet80211's cipher ops. encap is at +0x10; the only reference is inside
 * the same object, so a link wrap does not see it. */
typedef int (*ccmp_encap_fn)(void *key, void *frame, int keyid);
extern struct {
    uint32_t pad[4];
    ccmp_encap_fn encap;
    void *decap;
} ccmp;
static ccmp_encap_fn s_real_ccmp_encap;
/* Only the Wi-Fi task accesses this state. Preserve PN when the same TK is
 * reinstalled/refreshed, including a reconnect during this firmware boot. */
static uint8_t s_tx_pn_key[16];
static bool s_tx_pn_key_set;
static uint64_t s_next_tx_pn;
/* ccmp_encrypt is called with key+168 as the Temporal Key. Returns where the
 * installed TK already sits, and copies it to +168 when it is absent. */
static int align_tk(uint8_t *key, int *planted)
{
    uint8_t tk[16];
    uint8_t set = 0;
    *planted = 0;
    if (xSemaphoreTake(s_cfg_lock, 0) != pdTRUE) {
        return -3;
    }
    set = s_sta.tk_set;
    if (set) {
        memcpy(tk, s_sta.tk, sizeof(tk));
    }
    xSemaphoreGive(s_cfg_lock);
    if (!set) {
        return -2;
    }
    /* ppInstallKey allocates 168 + key_len bytes; scanning past offset 168
     * would read beyond this 16-byte CCMP key's allocation. */
    int found = memcmp(key + 168, tk, sizeof(tk)) == 0 ? 168 : -1;
    if (found != 168) {
        memcpy(key + 168, tk, sizeof(tk));
        *planted = 1;
    }
    if (!s_tx_pn_key_set || memcmp(s_tx_pn_key, tk, sizeof(tk)) != 0) {
        memcpy(s_tx_pn_key, tk, sizeof(tk));
        s_tx_pn_key_set = true;
        s_next_tx_pn = 1;
    }
    memset(tk, 0, sizeof(tk));
    return found;
}

static int hooked_ccmp_encap(void *key, void *frame, int keyid)
{
    static uint8_t logged;
    int where = -3;
    int planted = 0;
    if (key == NULL) {
        return 0;
    }
    where = align_tk(key, &planted);
    if ((where < 0 && !planted) || s_next_tx_pn >= (1ULL << 48)) {
        send_event_nb("sta tx dropped key unavailable or PN exhausted");
        return 0;
    }
    /* IDF's ccmp_encap adds 3 before writing the CCMP header. Reserve one
     * fresh PN for EVERY attempt; key refreshes must not restart it at 1. */
    uint64_t stored = s_next_tx_pn++ -
#if CONFIG_IDF_TARGET_ESP32
        1; // Verified ESP32 IDF5.5.6 ccmp_encap: counter+152 increments by one.
#else
        3;
#endif
    memcpy((uint8_t *)key + 152, &stored, sizeof(stored));
    int rc = s_real_ccmp_encap(key, frame, keyid);
    if (logged < 4) {
        char msg[96];
        uint8_t hdr[8] = {0};
        uint8_t *desc = *(uint8_t **)((uint8_t *)frame + 4);
        uint8_t *data = desc ? *(uint8_t **)(desc + 4) : NULL;
        if (data != NULL) {
            memcpy(hdr, data, sizeof(hdr));
        }
        logged++;
        snprintf(msg, sizeof(msg),
                 "sta ccmp encap n=%u keyid=%d rc=%d tkoff=%d planted=%d ccmp=%02x%02x%02x%02x%02x%02x%02x%02x",
                 logged, keyid, rc, where, planted,
                 hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7]);
        send_event_nb(msg);
    }
    return rc;
}
static void hook_ccmp(void)
{
    if (ccmp.encap != NULL && ccmp.encap != hooked_ccmp_encap) {
        s_real_ccmp_encap = ccmp.encap;
        ccmp.encap = hooked_ccmp_encap;
    }
}

/* The Wi-Fi library calls this with hdrlen 24 even for QoS data. A real
 * GameShare guest uses a 26-byte QoS header, and a 24-byte header makes the
 * CCMP MIC fail. The key itself is not logged. */
#if !CONFIG_IDF_TARGET_ESP32
extern uint8_t *__real_ccmp_encrypt(const uint8_t *tk, uint8_t *frame, size_t len,
                                    size_t hdrlen, uint8_t *pn, int keyid,
                                    size_t *encrypted_len);
extern uint8_t *ccmp_decrypt(const uint8_t *tk, const uint8_t *hdr, const uint8_t *data,
                             size_t data_len, size_t *decrypted_len, int espnow_pkt);

uint8_t *__wrap_ccmp_encrypt(const uint8_t *tk, uint8_t *frame, size_t len,
                             size_t hdrlen, uint8_t *pn, int keyid,
                             size_t *encrypted_len)
{
    size_t use = hdrlen;
    int qos = frame != NULL && len >= 26 && (frame[0] & 0x80) != 0 &&
              (frame[0] & 0x0C) == 0x08;
    if (qos && hdrlen == 24) {
        use = 26;
    }
    uint8_t *out = __real_ccmp_encrypt(tk, frame, len, use, pn, keyid, encrypted_len);
    static uint8_t n;
    if (n < 4) {
        int mic = -1;
        if (out != NULL && encrypted_len != NULL && *encrypted_len > use) {
            size_t plain_len = 0;
            uint8_t *plain = ccmp_decrypt(tk, out, out + use,
                                          *encrypted_len - use, &plain_len, 0);
            mic = plain != NULL;
            if (plain != NULL) {
                free(plain);
            }
        }
        char msg[96];
        n++;
        snprintf(msg, sizeof(msg),
                 "sta ccmp_encrypt n=%u hdr=%u use=%u len=%u fc=%02x pn0=%u mic=%d",
                 n, (unsigned)hdrlen, (unsigned)use, (unsigned)len,
                 frame ? frame[0] : 0, pn ? pn[0] : 0, mic);
        send_event_nb(msg);
    }
    return out;
}
#else
// ESP32 uses an OS adapter function pointer for ccmp_encrypt. Patch only the
// verified key slot/counter in its public-object caller; never reuse C5 ABI.
extern int __real_ieee80211_ccmp_encrypt(void *frame, void *key);
int __wrap_ieee80211_ccmp_encrypt(void *frame, void *key)
{
    int planted;
    if (frame == NULL || key == NULL || (align_tk(key, &planted) < 0 && !planted)) {
        return -1;
    }
    if (s_next_tx_pn >= (1ULL << 48)) {
        return -1;
    }
    uint64_t counter = s_next_tx_pn++ - 1;
    memcpy((uint8_t *)key + 152, &counter, sizeof(counter));
    return __real_ieee80211_ccmp_encrypt(frame, key);
}
static esp_ccmp_encrypt_t s_basic_encrypt;
static uint8_t *basic_encrypt(const uint8_t *tk, uint8_t *frame, size_t len,
                              size_t hdrlen, uint8_t *pn, int keyid, size_t *outlen)
{
    if (frame && len >= 26 && (frame[0] & 0x8c) == 0x88 && hdrlen == 24) hdrlen = 26;
    return s_basic_encrypt(tk, frame, len, hdrlen, pn, keyid, outlen);
}
#endif

/* Called from another object, so --wrap reaches it. The key is not logged. */
extern int __real_ieee80211_crypto_encap(void *node, void *m);
int __wrap_ieee80211_crypto_encap(void *node, void *m)
{
    int rc = __real_ieee80211_crypto_encap(node, m);
    static uint8_t logged;
    if (logged < 12) {
        char msg[48];
        logged++;
        snprintf(msg, sizeof(msg), "sta crypto encap n=%u ok=%d", logged, rc != 0);
        send_event_nb(msg);
    }
    return rc;
}

static int install_ldn_keys(uint8_t bssid[6], const uint8_t tk[16], bool open_port)
{
    /* ppInstallKey copies 8 bytes to the software counter in IDF 5.5.6. */
    uint8_t seq[8] = {0};
    uint8_t key[16];
    memcpy(key, tk, 16);
    hook_ccmp();
    int rc = esp_wifi_set_sta_key_internal(WIFI_WPA_ALG_CCMP, bssid, 0, 1, seq, sizeof(seq),
                                           key, sizeof(key),
                                           KEY_FLAG_PAIRWISE | KEY_FLAG_RX | KEY_FLAG_TX);
    if (rc != 0) {
        send_event("sta key pairwise rc=%d", rc);
        return rc;
    }
    rc = esp_wifi_set_sta_key_internal(WIFI_WPA_ALG_CCMP, bssid, 1, 0, seq, sizeof(seq),
                                       key, sizeof(key), KEY_FLAG_GROUP | KEY_FLAG_RX);
    if (rc != 0) {
        send_event("sta key group rc=%d", rc);
        return rc;
    }
    hook_ccmp();
    if (open_port) {
        esp_wifi_auth_done_internal();
    }
    send_event("sta tk installed cipher pair=%u group=%u",
               esp_wifi_sta_get_pairwise_cipher_internal(),
               esp_wifi_sta_get_group_cipher_internal());
    return 0;
}

/* Association response, status 0, from this BSSID. Drains the mgmt queue. */
static bool take_assoc_success(const uint8_t bssid[6])
{
    bool ok = false;
    uint8_t idx;
    if (s_mgmt_q == NULL) {
        return false;
    }
    while (xQueueReceive(s_mgmt_q, &idx, 0) == pdTRUE) {
        if (idx < MGMT_SLOTS) {
            mgmt_item_t *item = &s_mgmt_pool[idx];
            if (!ok && item->subtype == 1 && item->len >= 30 && from_bssid(item->data, bssid)) {
                uint16_t status = (uint16_t)(item->data[26] | (item->data[27] << 8));
                ok = status == 0;
            }
            s_mgmt_used[idx] = 0;
        }
    }
    return ok;
}

/* Returns 0 and sets s_native_up when the stack reports connected.
 * Otherwise the disconnect reason, -1 on timeout, -2 on a local API error.
 * tk may be NULL; when set, it is installed as soon as association succeeds
 * and the driver's 4-way timer is cancelled. */
static int native_connect(const uint8_t ascii[32], const uint8_t bssid[6],
                          uint8_t channel, bool wpa, int wait_s, const uint8_t *tk)
{
    s_disc_reason = -1;
    s_native_up = 0;
    wifi_config_t wcfg;
    memset(&wcfg, 0, sizeof(wcfg));
    memcpy(wcfg.sta.ssid, ascii, 32);
    if (wpa) {
        memcpy(wcfg.sta.password, "password", 8);
    }
    wcfg.sta.bssid_set = true;
    memcpy(wcfg.sta.bssid, bssid, 6);
    wcfg.sta.channel = channel;
    wcfg.sta.scan_method = WIFI_FAST_SCAN;
    wcfg.sta.sort_method = WIFI_CONNECT_AP_BY_SIGNAL;
    wcfg.sta.threshold.rssi = -90;
    wcfg.sta.threshold.authmode = WIFI_AUTH_OPEN;

    esp_err_t err = esp_wifi_set_config(WIFI_IF_STA, &wcfg);
    if (err != ESP_OK) {
        send_event("sta join fail set_config %s", esp_err_to_name(err));
        return -2;
    }
    err = esp_wifi_connect();
    if (err != ESP_OK) {
        send_event("sta join fail connect %s", esp_err_to_name(err));
        return -2;
    }
    send_event("sta native connecting %s tk=%d", wpa ? "wpa2" : "open", tk != NULL);
    int64_t t0 = esp_timer_get_time();
    int64_t deadline = t0 + (int64_t)wait_s * 1000000LL;
    bool keys_done = tk == NULL;
    bool tried_late = false;
    while (esp_timer_get_time() < deadline && !s_sta_cancel) {
        if (!keys_done) {
            bool assoc = take_assoc_success(bssid);
            bool late = !tried_late && esp_timer_get_time() - t0 > 1200000;
            if (assoc || late) {
                tried_late = tried_late || late;
                if (install_ldn_keys((uint8_t *)bssid, tk, true) == 0) {
                    keys_done = true;
                }
            }
        }
        if (s_native_up) {
            return 0;
        }
        if (s_disc_reason >= 0) {
            return s_disc_reason;
        }
        vTaskDelay(pdMS_TO_TICKS(50));
    }
    esp_wifi_disconnect();
    return -1;
}

static void do_join(void)
{
    if (s_sta_cancel || !s_sta_ready) {
        s_sta_state = STA_IDLE;
        return;
    }
    sta_cfg_t cfg;
    xSemaphoreTake(s_cfg_lock, portMAX_DELAY);
    cfg = s_sta;
    xSemaphoreGive(s_cfg_lock);

    uint8_t mac[6];
    esp_err_t e = esp_wifi_get_mac(WIFI_IF_STA, mac);
    if (e != ESP_OK) {
        send_event("sta join fail get_mac %s", esp_err_to_name(e));
        s_sta_state = STA_IDLE;
        return;
    }
    memcpy(s_own_mac, mac, 6);
    s_bssid_known = cfg.bssid_any ? 0 : 1;
    if (!cfg.bssid_any) {
        memcpy(s_join_bssid, cfg.bssid, 6);
    }
    __sync_synchronize();

    uint8_t ascii[32];
    format_ssid_ascii(cfg.ssid, ascii);
    flush_mgmt();
    s_key_logged = 0;
    s_sta_state = STA_PROBING;
    send_event("sta probing ch=%u tk=%u", cfg.channel, cfg.tk_set);

    uint8_t bssid[6];
    int have_bssid = !cfg.bssid_any;
    if (have_bssid) {
        memcpy(bssid, cfg.bssid, 6);
    }

    bool probed = false;
    for (int attempt = 0; attempt < 8 && !s_sta_cancel; attempt++) {
        uint8_t frame[160];
        int n = build_probe(frame, mac, ascii, cfg.channel);
        if (!tx_mgmt(frame, n, "probe")) {
            s_sta_state = STA_IDLE;
            send_event("sta join fail probe tx");
            return;
        }
        int64_t until = esp_timer_get_time() + 250 * 1000;
        while (esp_timer_get_time() < until && !s_sta_cancel) {
            mgmt_item_t item;
            int left_ms = (int)((until - esp_timer_get_time()) / 1000);
            if (left_ms < 1) {
                left_ms = 1;
            }
            if (!wait_mgmt(5, left_ms, &item)) {
                break;
            }
            if (have_bssid && !from_bssid(item.data, bssid)) {
                continue;
            }
            /* Probe response: 24-byte header + 12-byte fixed fields, then IEs. */
            if (item.len >= 36 && ie_has_ssid(item.data, item.len, 36, ascii)) {
                memcpy(bssid, item.data + 16, 6);
                have_bssid = true;
                probed = true;
                break;
            }
        }
        if (probed) {
            break;
        }
    }
    if (!probed) {
        s_sta_state = STA_IDLE;
        if (!s_sta_cancel) {
            send_event("sta join fail probe timeout");
        }
        return;
    }
    remember_bssid(bssid);
    char bss[18];
    mac_fmt(bssid, bss);
    send_event("sta probe ok bssid=%s", bss);

    /* esp_wifi_80211_tx accepts probe requests and rejects authentication.
     * The station stack sends auth/assoc. LDN has no EAPOL, so a WPA2
     * handshake timeout (reason 204) means the AP already accepted assoc. */
    static const uint8_t nintendo_ie[] = {0xdd, 0x06, 0x00, 0x22, 0xaa, 0x10, 0x01, 0x01};
    esp_wifi_set_vendor_ie(true, WIFI_VND_IE_TYPE_ASSOC_REQ, WIFI_VND_IE_ID_0, nintendo_ie);
    s_sta_state = STA_AUTH;
    int reason = native_connect(ascii, bssid, cfg.channel, true, 15,
                                cfg.tk_set ? cfg.tk : NULL);
    if (s_native_up) {
        s_sta_state = STA_LINKED;
        return;
    }
    s_sta_state = STA_IDLE;
    if (s_sta_cancel) {
        return;
    }
    if (reason == 15 || reason == 204 || reason == 205) {
        send_event("sta join fail handshake reason=%d (assoc reached)", reason);
    } else if (reason < 0) {
        send_event("sta join fail native timeout");
    } else {
        send_event("sta join fail native reason=%d", reason);
    }
}

static void on_wifi_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base != WIFI_EVENT) {
        return;
    }
    if (id == WIFI_EVENT_STA_CONNECTED) {
        const wifi_event_sta_connected_t *e = data;
        s_native_up = 1;
        send_event("sta native connected aid=%u ch=%u", e->aid, e->channel);
        send_event("sta linked aid=%u", e->aid);
    } else if (id == WIFI_EVENT_STA_DISCONNECTED) {
        const wifi_event_sta_disconnected_t *e = data;
        s_native_up = 0;
        s_disc_reason = e->reason;
        send_event("sta native disconnect reason=%u", e->reason);
        if (s_sta_state == STA_LINKED) {
            s_sta_state = STA_IDLE;
            send_event("sta link dropped reason=%u", e->reason);
        }
    }
}

static void do_leave(void)
{
    esp_wifi_disconnect();
    uint8_t st = s_sta_state;
    if (st == STA_LINKED && s_bssid_known) {
        uint8_t mac[6];
        if (esp_wifi_get_mac(WIFI_IF_STA, mac) == ESP_OK) {
            tx_disassoc(mac, s_join_bssid);
        }
    }
    s_sta_state = STA_IDLE;
    s_bssid_known = 0;
    s_sta_cancel = 0;
    send_event("sta idle");
}

static void sta_task(void *arg)
{
    for (;;) {
        uint8_t req = 0;
        if (xQueueReceive(s_sta_q, &req, portMAX_DELAY) != pdTRUE) {
            continue;
        }
        if (req == REQ_LEAVE) {
            do_leave();
        } else if (req == REQ_JOIN) {
            do_join();
        }
    }
}

static void sta_start(void)
{
    s_mgmt_q = xQueueCreate(MGMT_SLOTS, sizeof(uint8_t));
    s_sta_q = xQueueCreate(4, sizeof(uint8_t));
    configASSERT(s_mgmt_q && s_sta_q);
    xTaskCreate(sta_task, "sta", 8192, NULL, 8, NULL);
}

/* Switch the radio to STA so esp_wifi_80211_tx is allowed. Promiscuous
 * receive stays available; monitor mode puts the radio back to NULL. */
static esp_err_t ensure_sta_mode(void)
{
    wifi_mode_t mode = WIFI_MODE_NULL;
    esp_wifi_get_mode(&mode);
    if (mode != WIFI_MODE_STA) {
        esp_err_t e = esp_wifi_set_mode(WIFI_MODE_STA);
        if (e != ESP_OK) {
            esp_wifi_stop();
            e = esp_wifi_set_mode(WIFI_MODE_STA);
            if (e != ESP_OK) {
                return e;
            }
            e = esp_wifi_start();
            if (e != ESP_OK) {
                return e;
            }
            esp_wifi_set_promiscuous_rx_cb(promisc_cb);
            apply_filter();
        }
    }
    esp_wifi_set_ps(WIFI_PS_NONE);
    return esp_wifi_internal_reg_rxcb(WIFI_IF_STA, sta_ethernet_rx);
}

/* `sta ...` subcommands. `sta set` stores the session; `sta join` runs the
 * open-system handshake. The TK is stored for a later CCMP phase and is not
 * required to associate. */
static void handle_sta(char *sub, char *a1, char *a2, char *a3, char *a4, char *err, size_t errlen)
{
    if (!strcasecmp(sub, "mac")) {
        uint8_t mac[6];
        esp_err_t e = esp_wifi_get_mac(WIFI_IF_STA, mac);
        if (e != ESP_OK) {
            snprintf(err, errlen, "get_mac %s", esp_err_to_name(e));
        } else {
            send_text("ok sta mac %02x:%02x:%02x:%02x:%02x:%02x",
                      mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        }
    } else if (!strcasecmp(sub, "macset") && a1) {
        /* 802.11 injection only accepts the factory MAC in address 2.
         * "factory" restores the address burned into efuse. */
        uint8_t mac[6];
        if (!strcasecmp(a1, "factory")) {
            esp_err_t e = esp_read_mac(mac, ESP_MAC_WIFI_STA);
            if (e != ESP_OK) {
                snprintf(err, errlen, "read_mac %s", esp_err_to_name(e));
                return;
            }
        } else if (!parse_mac(a1, mac) || (mac[0] & 1)) {
            snprintf(err, errlen, "bad mac %s", a1);
            return;
        }
        esp_err_t e = ensure_sta_mode();
        if (e != ESP_OK) {
            snprintf(err, errlen, "sta mode %s", esp_err_to_name(e));
            return;
        }
        e = esp_wifi_set_mac(WIFI_IF_STA, mac);
        if (e != ESP_OK) {
            snprintf(err, errlen, "set_mac %s", esp_err_to_name(e));
            return;
        }
        send_text("ok sta mac %02x:%02x:%02x:%02x:%02x:%02x",
                  mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    } else if (!strcasecmp(sub, "set") && a1 && a2 && a3) {
        sta_cfg_t cfg;
        memset(&cfg, 0, sizeof(cfg));
        size_t n = parse_hex(a1, cfg.ssid, 16);
        if (n != 16) {
            snprintf(err, errlen, "ssidhex must be 16 bytes (32 hex digits)");
            return;
        }
        cfg.ssid_len = 16;
        if (strcasecmp(a2, "*") == 0) {
            cfg.bssid_any = 1;
        } else if (!parse_mac(a2, cfg.bssid)) {
            snprintf(err, errlen, "bad bssid %s", a2);
            return;
        }
        int ch = atoi(a3);
        if (!channel_valid(ch)) {
            snprintf(err, errlen, "bad channel %s", a3);
            return;
        }
        cfg.channel = (uint8_t)ch;
        if (a4 && strcasecmp(a4, "-") != 0 && strcasecmp(a4, "none") != 0) {
            if (parse_hex(a4, cfg.tk, sizeof(cfg.tk)) != 16) {
                snprintf(err, errlen, "bad tkhex (need 16 bytes, or -)");
                return;
            }
            cfg.tk_set = 1;
        }
        if (s_sta_state != STA_IDLE) {
            snprintf(err, errlen, "sta active");
            return;
        }
        s_sta = cfg;
        s_sta_ready = 1;
        send_text("ok sta set ssid_len=16 bssid=%s ch=%u tk=%s",
                  cfg.bssid_any ? "*" : a2, cfg.channel, cfg.tk_set ? "yes" : "no");
        send_event("sta configured ch=%u tk=%u", cfg.channel, cfg.tk_set);
    } else if (!strcasecmp(sub, "rekey") && a1) {
        /* Replace the CCMP key on a live link. The hex key is not logged. */
        uint8_t tk[16];
        if (parse_hex(a1, tk, sizeof(tk)) != sizeof(tk)) {
            snprintf(err, errlen, "bad tkhex");
            return;
        }
        /* handle_command already holds s_cfg_lock. Taking it again deadlocks. */
        memcpy(s_sta.tk, tk, sizeof(tk));
        s_sta.tk_set = 1;
        if (s_sta_state == STA_LINKED) {
            if (install_ldn_keys(s_join_bssid, tk, false) != 0) {
                memset(tk, 0, sizeof(tk));
                snprintf(err, errlen, "rekey failed");
                return;
            }
        }
        memset(tk, 0, sizeof(tk));
        s_key_logged = 0;
        send_text("ok sta rekey");
    } else if (!strcasecmp(sub, "join")) {
        if (!s_sta_ready) {
            snprintf(err, errlen, "sta set first");
            return;
        }
        if (s_sta_state != STA_IDLE) {
            snprintf(err, errlen, "sta busy");
            return;
        }
        if (s_sta_q == NULL) {
            snprintf(err, errlen, "sta task down");
            return;
        }
        esp_err_t e = ensure_sta_mode();
        if (e != ESP_OK) {
            snprintf(err, errlen, "sta mode %s", esp_err_to_name(e));
            return;
        }
        s_hop_list[0] = s_sta.channel;
        s_hop_count = 1;
        e = tune(s_sta.channel);
        if (e != ESP_OK) {
            snprintf(err, errlen, "tune %s", esp_err_to_name(e));
            return;
        }
        s_filter_mask = WIFI_PROMIS_FILTER_MASK_MGMT | WIFI_PROMIS_FILTER_MASK_DATA;
        apply_filter();
        esp_wifi_set_promiscuous(true);
        s_capturing = true;
        s_mode = MODE_SOFTSTA;
        s_sta_cancel = 0;
        s_wake_hop = 1;
        uint8_t req = REQ_JOIN;
        if (xQueueSend(s_sta_q, &req, 0) != pdTRUE) {
            snprintf(err, errlen, "sta queue full");
            return;
        }
        send_text("ok sta join ch=%u", s_sta.channel);
    } else if (!strcasecmp(sub, "leave")) {
        s_sta_cancel = 1;
        if (s_sta_q == NULL) {
            snprintf(err, errlen, "sta task down");
            return;
        }
        uint8_t req = REQ_LEAVE;
        if (xQueueSend(s_sta_q, &req, 0) != pdTRUE) {
            snprintf(err, errlen, "sta queue full");
            return;
        }
        send_text("ok sta leave");
    } else if (!strcasecmp(sub, "loop") && a1) {
        int on = (strcasecmp(a1, "1") == 0 || strcasecmp(a1, "on") == 0) ? 1 :
                 (strcasecmp(a1, "0") == 0 || strcasecmp(a1, "off") == 0) ? 0 : -1;
        if (on < 0) {
            snprintf(err, errlen, "loop needs 0/1 (on/off)");
            return;
        }
        s_h2d_echo = (uint8_t)on;
        send_text("ok sta loop %d", on);
    } else if (!strcasecmp(sub, "stats")) {
        send_text("sta h2d=%lu d2h=%lu drop=%lu echo=%u mode=%s ready=%u state=%s seq=%lu",
                  (unsigned long)s_h2d_frames, (unsigned long)s_d2h_frames,
                  (unsigned long)s_data_dropped, s_h2d_echo,
                  s_mode == MODE_SOFTSTA ? "softsta" : "monitor", s_sta_ready,
                  sta_state_name(s_sta_state), (unsigned long)s_rx_seq);
    } else {
        snprintf(err, errlen, "unknown sta subcommand: %s", sub ? sub : "(none)");
    }
}

static void handle_command(port_t port, char *line)
{
    s_active_port = port;

    char *cmd = strtok(line, " \t");
    if (!cmd) {
        return;
    }
    char *arg1 = strtok(NULL, " \t");
    char *arg2 = strtok(NULL, " \t");
    char *arg3 = strtok(NULL, " \t");
    char *arg4 = strtok(NULL, " \t");
    char err[64] = "";

    xSemaphoreTake(s_cfg_lock, portMAX_DELAY);

    if (!strcasecmp(cmd, "hello")) {
        send_text("ok hello c5sniffer %s port=%s t=%llu reset=%d heap=%lu minheap=%lu", FW_VERSION,
                  port == PORT_USB ? "usb" : port == PORT_FX2 ? "fx2 board=basic" :
#if CONFIG_IDF_TARGET_ESP32
                  "uart board=basic",
#else
                  "uart",
#endif
                  (unsigned long long)esp_timer_get_time(),
                  (int)esp_reset_reason(), (unsigned long)esp_get_free_heap_size(),
                  (unsigned long)esp_get_minimum_free_heap_size());
#if CONFIG_SNIFFER_USE_UART
    } else if (!strcasecmp(cmd, "baud") && arg1) {
        char *end = NULL;
        unsigned long baud = strtoul(arg1, &end, 10);
        if (port != PORT_UART || s_capturing || s_sta_state != STA_IDLE) {
            snprintf(err, sizeof(err), "baud requires idle uart");
        } else if (!end || *end || baud < 1000000 || baud > SOC_UART_BITRATE_MAX || arg2) {
            snprintf(err, sizeof(err), "baud must be 1000000..%u", SOC_UART_BITRATE_MAX);
        } else {
            uint8_t request[5] = {INTERNAL_UART_BAUD};
            uint32_t rate = (uint32_t)baud;
            memcpy(request + 1, &rate, sizeof(rate));
            if (xRingbufferSend(s_ring, request, sizeof(request), pdMS_TO_TICKS(100)) != pdTRUE) {
                snprintf(err, sizeof(err), "baud queue full");
            }
        }
#endif
    } else if (!strcasecmp(cmd, "start")) {
        s_rx_total = s_rx_captured = s_rx_filtered = s_rx_dropped = 0;
        apply_filter();
        esp_wifi_set_promiscuous(true);
        s_capturing = true;
        send_text("ok start t=%llu ch=%u", (unsigned long long)esp_timer_get_time(), s_cur_channel);
    } else if (!strcasecmp(cmd, "stop")) {
        if (s_sta_state != STA_IDLE && s_sta_q) {
            s_sta_cancel = 1;
            uint8_t req = REQ_LEAVE;
            xQueueSend(s_sta_q, &req, 0);
        }
        s_capturing = false;
        esp_wifi_set_promiscuous(false);
        send_text("ok stop");
    } else if (!strcasecmp(cmd, "ch") && arg1) {
        int ch = atoi(arg1);
        if (s_sta_state != STA_IDLE) {
            snprintf(err, sizeof(err), "sta active");
            ch = -1;
        }
        esp_err_t e;
        if (ch < 0) {
            /* sta active: err already set */
        } else if (!channel_valid(ch)) {
            snprintf(err, sizeof(err), "bad channel %s", arg1);
        } else if ((e = tune((uint8_t)ch)) != ESP_OK) {
            snprintf(err, sizeof(err), "set_channel %d: %s", ch, esp_err_to_name(e));
        } else {
            s_hop_list[0] = (uint8_t)ch;
            s_hop_count = 1;
            send_text("ok ch %d", ch);
        }
    } else if (!strcasecmp(cmd, "hop") && arg1 && arg2) {
        int dwell = atoi(arg1);
        if (s_sta_state != STA_IDLE) {
            snprintf(err, sizeof(err), "sta active");
        } else if (dwell < 10 || dwell > 60000) {
            snprintf(err, sizeof(err), "dwell must be 10..60000 ms");
        } else if (parse_channel_list(arg2, err, sizeof(err))) {
            s_dwell_ms = (uint32_t)dwell;
            tune(s_hop_list[0]);
            send_text("ok hop %d ms, %u channels", dwell, s_hop_count);
        }
    } else if (!strcasecmp(cmd, "filter") && arg1) {
        uint32_t m = strtoul(arg1, NULL, 0) & 0x7;
        s_filter_mask = (m & 1 ? WIFI_PROMIS_FILTER_MASK_MGMT : 0) |
                        (m & 2 ? WIFI_PROMIS_FILTER_MASK_CTRL : 0) |
                        (m & 4 ? WIFI_PROMIS_FILTER_MASK_DATA : 0);
        apply_filter();
        send_text("ok filter %lu", (unsigned long)m);
    } else if (!strcasecmp(cmd, "snap") && arg1) {
        int n = atoi(arg1);
        s_snaplen = (uint16_t)(n < 0 ? 0 : (n > MAX_CAPLEN ? MAX_CAPLEN : n));
        send_text("ok snap %u", s_snaplen);
    } else if (!strcasecmp(cmd, "rssi") && arg1) {
        int r = atoi(arg1);
        s_min_rssi = (int8_t)(r < -128 ? -128 : (r > 0 ? 0 : r));
        send_text("ok rssi %d", s_min_rssi);
    } else if (!strcasecmp(cmd, "mac") && arg1) {
        uint8_t mac[6];
        if (!strcasecmp(arg1, "clear")) {
            s_mac_count = 0;
            send_text("ok mac clear");
        } else if (!parse_mac(arg1, mac)) {
            snprintf(err, sizeof(err), "bad mac %s", arg1);
        } else if (s_mac_count >= MAX_MACS) {
            snprintf(err, sizeof(err), "mac list full (max %d)", MAX_MACS);
        } else {
            /* Fill the slot before publishing the new count to the RX callback. */
            memcpy(s_macs[s_mac_count], mac, 6);
            s_mac_count++;
            send_text("ok mac %s (%u)", arg1, s_mac_count);
        }
    } else if (!strcasecmp(cmd, "country") && arg1) {
        esp_err_t e = esp_wifi_set_country_code(arg1, false);
        if (e != ESP_OK) {
            snprintf(err, sizeof(err), "country %s: %s", arg1, esp_err_to_name(e));
        } else {
            send_text("ok country %s", arg1);
        }
    } else if (!strcasecmp(cmd, "bench") && arg1) {
        int ms = atoi(arg1);
        if (s_capturing || ms < 100 || ms > 60000) {
            snprintf(err, sizeof(err), "bench needs 100..60000 ms and capture stopped");
        } else {
            xTaskCreate(bench_task, "bench", 3072, (void *)(uintptr_t)ms, 5, NULL);
        }
    } else if (!strcasecmp(cmd, "stats")) {
        send_text("stats rx=%lu cap=%lu filt=%lu drop=%lu ch=%u",
                  (unsigned long)s_rx_total, (unsigned long)s_rx_captured,
                  (unsigned long)s_rx_filtered, (unsigned long)s_rx_dropped, s_cur_channel);
    } else if (!strcasecmp(cmd, "mode") && arg1) {
        if (!strcasecmp(arg1, "monitor")) {
            s_sta_cancel = 1;
            if (s_sta_state != STA_IDLE && s_sta_q) {
                uint8_t req = REQ_LEAVE;
                xQueueSend(s_sta_q, &req, 0);
            }
            s_mode = MODE_MONITOR;
            s_capturing = false;
            s_h2d_echo = 0;
            esp_wifi_set_promiscuous(false);
            wifi_mode_t mode = WIFI_MODE_NULL;
            if (esp_wifi_get_mode(&mode) == ESP_OK && mode != WIFI_MODE_NULL) {
                if (esp_wifi_set_mode(WIFI_MODE_NULL) != ESP_OK) {
                    esp_wifi_stop();
                    esp_wifi_set_mode(WIFI_MODE_NULL);
                    esp_wifi_start();
                    esp_wifi_set_promiscuous_rx_cb(promisc_cb);
                }
            }
            send_text("ok mode monitor");
        } else if (!strcasecmp(arg1, "softsta")) {
            s_mode = MODE_SOFTSTA;
            send_text("ok mode softsta");
        } else {
            snprintf(err, sizeof(err), "bad mode %s", arg1);
        }
    } else if (!strcasecmp(cmd, "sta")) {
        handle_sta(arg1, arg2, arg3, arg4, strtok(NULL, " \t"), err, sizeof(err));
    } else {
        snprintf(err, sizeof(err), "unknown command: %s", cmd);
    }

    xSemaphoreGive(s_cfg_lock);

    if (err[0]) {
        send_text("err %s", err);
    }
    if (!strcasecmp(cmd, "ch") || !strcasecmp(cmd, "hop") || s_wake_hop) {
        s_wake_hop = 0;
        xTaskNotifyGive(s_hop_task);
    }
}

typedef int (*port_read_fn)(uint8_t *buf, size_t len);

#if CONFIG_SNIFFER_USE_UART
static int uart_read(uint8_t *buf, size_t len)
{
    return uart_read_bytes(UART_NUM_0, buf, len, pdMS_TO_TICKS(20));
}
#endif
#if CONFIG_SNIFFER_USE_USB_JTAG
static int usb_read(uint8_t *buf, size_t len)
{
    return usb_serial_jtag_read_bytes(buf, len, pdMS_TO_TICKS(20));
}
#endif

typedef struct {
    port_t port;
    port_read_fn read;
} reader_args_t;

/* Byte-stream demultiplexer: ASCII command lines ('\n' terminated) and binary
 * H2D frames (A5 5A ...) share the stream. Sync on A5 5A, then accumulate the
 * header + payload + CRC; any byte that does not fit a valid frame falls back
 * into the text accumulator, so command traffic keeps working at all times. */

typedef enum { RS_TEXT = 0, RS_SYNC, RS_HDR, RS_PAYLOAD, RS_CRC } rs_state_t;

typedef struct {
    port_t port;
    port_read_fn read;

    rs_state_t state;
    char line[400];
    size_t n;
    uint8_t hdr[3];
    size_t hn;
    uint16_t plen;
    uint8_t payload[MAX_H2D_LEN];
    size_t written;
    uint8_t crc[2];
    size_t crcn;
} reader_ctx_t;

static void text_byte(reader_ctx_t *st, uint8_t b)
{
    if (b == '\n' || b == '\r') {
        if (st->n > 0) {
            st->line[st->n] = '\0';
            handle_command(st->port, st->line);
            st->n = 0;
        }
    } else if (st->n < sizeof(st->line) - 1) {
        st->line[st->n++] = (char)b;
    } else {
        st->n = 0; /* overlong line: discard */
    }
}

static void reader_feed(reader_ctx_t *st, const uint8_t *data, size_t len)
{
    for (size_t i = 0; i < len; i++) {
        uint8_t b = data[i];
        switch (st->state) {
        case RS_TEXT:
            if (b == 0xA5) {
                st->state = RS_SYNC;
            } else {
                text_byte(st, b);
            }
            break;
        case RS_SYNC:
            if (b == 0x5A) {
                st->state = RS_HDR;
                st->hn = 0;
            } else {
                st->state = RS_TEXT;
                text_byte(st, 0xA5);
                text_byte(st, b);
            }
            break;
        case RS_HDR:
            st->hdr[st->hn++] = b;
            if (st->hn == sizeof(st->hdr)) {
                uint16_t flen = (uint16_t)(st->hdr[1] | (st->hdr[2] << 8));
                if (st->hdr[0] != FRAME_TYPE_H2D_DATA || flen < DATA_HDR_SIZE ||
                        flen > MAX_H2D_LEN) {
                    st->state = RS_TEXT;   /* not a data frame: back to text */
                } else {
                    st->plen = flen;
                    st->written = 0;
                    st->state = RS_PAYLOAD;
                }
            }
            break;
        case RS_PAYLOAD:
            st->payload[st->written++] = b;
            if (st->written == st->plen) {
                st->state = RS_CRC;
                st->crcn = 0;
            }
            break;
        case RS_CRC:
            st->crc[st->crcn++] = b;
            if (st->crcn == 2) {
                uint16_t crc = crc16_update(0xFFFF, st->hdr, 3);
                crc = crc16_update(crc, st->payload, st->plen);
                uint16_t got = (uint16_t)(st->crc[0] | (st->crc[1] << 8));
                if (crc == got) {
                    handle_data_frame(st->payload, st->plen);
                }
                st->state = RS_TEXT;
            }
            break;
        }
    }
}

#if CONFIG_SNIFFER_USE_UART
static reader_ctx_t s_ctx_uart;
#endif
#if CONFIG_SNIFFER_USE_USB_JTAG
static reader_ctx_t s_ctx_usb;
#endif

static void reader_task(void *arg)
{
    reader_ctx_t *st = arg;
    uint8_t buf[512];
    int64_t last_rx = esp_timer_get_time();
    for (;;) {
        int got = st->read(buf, sizeof(buf));
        if (got > 0) {
            last_rx = esp_timer_get_time();
            reader_feed(st, buf, (size_t)got);
        } else {
            /* Recover after a PC write was interrupted or its port was closed.
             * A partial binary frame must not swallow subsequent hello lines. */
            if (st->state != RS_TEXT && esp_timer_get_time() - last_rx > 250000) {
                st->state = RS_TEXT;
                st->n = 0;
            }
            vTaskDelay(pdMS_TO_TICKS(5));
        }
    }
}

/* ------------------------------------------------------------------------- */
/* Init                                                                      */
/* ------------------------------------------------------------------------- */

static void wifi_init(void)
{
    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    ESP_ERROR_CHECK(err);
    ESP_ERROR_CHECK(esp_event_loop_create_default());

    wifi_init_config_t cfg = WIFI_INIT_CONFIG_DEFAULT();
#if CONFIG_IDF_TARGET_ESP32
    s_basic_encrypt = cfg.wpa_crypto_funcs.ccmp_encrypt;
    cfg.wpa_crypto_funcs.ccmp_encrypt = basic_encrypt;
#endif
    ESP_ERROR_CHECK(esp_wifi_init(&cfg));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_NULL));
    ESP_ERROR_CHECK(esp_wifi_set_country_code(CONFIG_SNIFFER_COUNTRY, false));
    ESP_ERROR_CHECK(esp_wifi_start());
    hook_ccmp();
    ESP_ERROR_CHECK(esp_event_handler_instance_register(WIFI_EVENT, ESP_EVENT_ANY_ID,
                                                        on_wifi_event, NULL, NULL));
#if !CONFIG_IDF_TARGET_ESP32
    ESP_ERROR_CHECK(esp_wifi_set_band_mode(WIFI_BAND_MODE_AUTO));
#endif
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous_rx_cb(promisc_cb));
#if !CONFIG_IDF_TARGET_ESP32
    ESP_ERROR_CHECK(esp_wifi_register_80211_tx_cb(on_80211_tx));
#endif
    apply_filter();

    s_hop_list[0] = CONFIG_SNIFFER_DEFAULT_CHANNEL;
    s_hop_count = 1;
    tune(CONFIG_SNIFFER_DEFAULT_CHANNEL);
}

void app_main(void)
{
    crc16_init();
    s_cfg_lock = xSemaphoreCreateMutex();
    s_ring = xRingbufferCreate(CONFIG_SNIFFER_RINGBUF_SIZE, RINGBUF_TYPE_NOSPLIT);
    configASSERT(s_cfg_lock && s_ring);

    wifi_init();
    sta_start();

    xTaskCreate(writer_task, "writer", 3072, NULL, 10, NULL);
    xTaskCreate(hop_task, "hop", 3072, NULL, 9, &s_hop_task);
#if CONFIG_BASIC_FX2
    ESP_ERROR_CHECK(fx2_fifo_init());
    s_active_port = PORT_FX2;
    send_event("basic link boot %s", FW_VERSION);
    static reader_ctx_t basic_reader;
    basic_reader.port = PORT_FX2;
    basic_reader.read = fx2_return_read;
    xTaskCreate(reader_task, "rd_fx2", 4096, &basic_reader, 8, NULL);
#endif

#if CONFIG_SNIFFER_USE_UART
    uart_config_t ucfg = {
        .baud_rate = CONFIG_SNIFFER_UART_BAUD,
        .data_bits = UART_DATA_8_BITS,
        .parity = UART_PARITY_DISABLE,
        .stop_bits = UART_STOP_BITS_1,
        .flow_ctrl = UART_HW_FLOWCTRL_DISABLE,
        .source_clk = UART_SCLK_DEFAULT,
    };
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_0, 1024, 16384, 0, NULL, 0));
    ESP_ERROR_CHECK(uart_param_config(UART_NUM_0, &ucfg));
    /* With the console disabled nothing else routes UART0 to its pins. */
    ESP_ERROR_CHECK(uart_set_pin(UART_NUM_0, U0TXD_GPIO_NUM, U0RXD_GPIO_NUM,
                                 UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE));
    s_ctx_uart.port = PORT_UART;
    s_ctx_uart.read = uart_read;
    xTaskCreate(reader_task, "rd_uart", 4096, &s_ctx_uart, 8, NULL);
#endif
#if CONFIG_SNIFFER_USE_USB_JTAG
    usb_serial_jtag_driver_config_t jcfg = {.tx_buffer_size = 32768, .rx_buffer_size = 1024};
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&jcfg));
    s_ctx_usb.port = PORT_USB;
    s_ctx_usb.read = usb_read;
    xTaskCreate(reader_task, "rd_usb", 4096, &s_ctx_usb, 8, NULL);
#endif

    const esp_timer_create_args_t targs = {.callback = stats_timer_cb, .name = "stats"};
    esp_timer_handle_t timer;
    ESP_ERROR_CHECK(esp_timer_create(&targs, &timer));
    ESP_ERROR_CHECK(esp_timer_start_periodic(timer, STATS_PERIOD_US));
}
