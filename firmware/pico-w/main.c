#include <stdio.h>
#include <string.h>
#include "pico/stdlib.h"
#include "pico/cyw43_arch.h"
#include "btstack.h"

// Diagnostic firmware: no strap connection, command writes, or cloud credentials.
static btstack_packet_callback_registration_t events;
static btstack_timer_source_t heartbeat;
static unsigned advertisements, whoop_advertisements, wifi_networks;
static bool bluetooth_ready;
static int wifi_scan_result(void *env, const cyw43_ev_scan_result_t *result) {
    (void)env;
    if (result) ++wifi_networks;
    return 0;
}
static void report(btstack_timer_source_t *timer) {
    printf("STATUS uptime_ms=%lu ble_ready=%d advertisements=%u whoop_advertisements=%u wifi_scan_results=%u\n",
           (unsigned long)to_ms_since_boot(get_absolute_time()), bluetooth_ready,
           advertisements, whoop_advertisements, wifi_networks);
    cyw43_arch_gpio_put(CYW43_WL_GPIO_LED_PIN, bluetooth_ready);
    btstack_run_loop_set_timer(timer, 10000);
    btstack_run_loop_add_timer(timer);
}
static void packet_handler(uint8_t type, uint16_t channel, uint8_t *packet, uint16_t size) {
    (void)channel; (void)size;
    if (type != HCI_EVENT_PACKET) return;
    switch (hci_event_packet_get_type(packet)) {
    case BTSTACK_EVENT_STATE:
        if (btstack_event_state_get_state(packet) == HCI_STATE_WORKING) {
            bluetooth_ready = true;
            printf("BLE ready: passive scan starting\n");
            gap_set_scan_parameters(0, 0x0060, 0x0030);
            gap_start_scan();
        }
        break;
    case GAP_EVENT_ADVERTISING_REPORT: {
        ++advertisements;
        uint8_t length = gap_event_advertising_report_get_data_length(packet);
        const uint8_t *data = gap_event_advertising_report_get_data(packet);
        ad_context_t it;
        for (ad_iterator_init(&it, length, data); ad_iterator_has_more(&it); ad_iterator_next(&it)) {
            uint8_t tag = ad_iterator_get_data_type(&it);
            if (tag != BLUETOOTH_DATA_TYPE_COMPLETE_LOCAL_NAME && tag != BLUETOOTH_DATA_TYPE_SHORTENED_LOCAL_NAME) continue;
            char name[40];
            unsigned n = ad_iterator_get_data_len(&it);
            if (n >= sizeof(name)) n = sizeof(name)-1;
            memcpy(name, ad_iterator_get_data(&it), n); name[n] = 0;
            if (strstr(name, "WHOOP") || strstr(name, "Whoop") || strstr(name, "4C0639073")) {
                ++whoop_advertisements;
                printf("WHOOP advertisement name=%s rssi=%d\n", name, (int)gap_event_advertising_report_get_rssi(packet));
            }
        }
        break;
    }
    default: break;
    }
}
int main(void) {
    stdio_init_all();
    sleep_ms(2000);
    printf("OpenWhoop Pico W radio bring-up 0.1.0; diagnostic only\n");
    if (cyw43_arch_init()) panic("CYW43 initialization failed");
    printf("CYW43 initialized\n");
    cyw43_arch_enable_sta_mode();
    cyw43_wifi_scan_options_t options = {0};
    cyw43_arch_lwip_begin();
    int scan_result = cyw43_wifi_scan(&cyw43_state, &options, NULL, wifi_scan_result);
    cyw43_arch_lwip_end();
    absolute_time_t deadline = make_timeout_time_ms(15000);
    while (!scan_result && cyw43_wifi_scan_active(&cyw43_state) && !time_reached(deadline)) sleep_ms(100);
    printf("Wi-Fi scan result=%d networks=%u\n", scan_result, wifi_networks);
    l2cap_init();
    sm_init();
    events.callback = packet_handler;
    hci_add_event_handler(&events);
    heartbeat.process = report;
    btstack_run_loop_set_timer(&heartbeat, 1000);
    btstack_run_loop_add_timer(&heartbeat);
    hci_power_control(HCI_POWER_ON);
    btstack_run_loop_execute();
    return 0;
}
