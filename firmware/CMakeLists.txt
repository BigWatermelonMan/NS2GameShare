cmake_minimum_required(VERSION 3.16)

include($ENV{IDF_PATH}/tools/cmake/project.cmake)
# Keep the build small: only pull in what the sniffer needs.
idf_build_set_property(MINIMAL_BUILD ON)
# output.o calls this. Wrapping it shows whether TX frames are encrypted.
idf_build_set_property(LINK_OPTIONS "-Wl,--wrap=ieee80211_crypto_encap" APPEND)
idf_build_set_property(LINK_OPTIONS "-Wl,--wrap=ccmp_encrypt" APPEND)
project(c5_sniffer)
