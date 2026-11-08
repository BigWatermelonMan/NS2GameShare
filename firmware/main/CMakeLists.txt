idf_component_register(SRCS "main.c"
                       INCLUDE_DIRS "."
                       PRIV_REQUIRES esp_wifi esp_event nvs_flash esp_timer esp_ringbuf
                                     esp_driver_uart esp_driver_usb_serial_jtag)
