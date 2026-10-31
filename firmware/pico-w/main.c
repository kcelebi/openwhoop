#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

#include "pico/cyw43_arch.h"
#include "pico/printf.h"
#include "pico/status_led.h"
#include "pico/time.h"

int main() {
    // Spike bring-up: prove toolchain + CYW43 (BLE radio) bring-up works.
    // Next: BLE central toward Whoop strap + forward over Wi-Fi webhook.
    printf("pico_w openwhoop relay: booting\n");

    if (cyw43_arch_init() != 0) {
        printf("pico_w: cyw43_arch_init failed\n");
        return 1;
    }
    printf("pico_w: cyw43_arch_init ok\n");

    status_led_init();
    status_led_set_state(true);

    while (true) {
        sleep_us(100000); // 100ms idle
    }
}
