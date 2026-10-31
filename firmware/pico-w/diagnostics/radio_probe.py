import sys, time, network, bluetooth, gc

print("Runtime:", sys.implementation)
print("Free heap before radio:", gc.mem_free())
wlan = network.WLAN(network.STA_IF)
wlan.active(True)
print("Wi-Fi active:", wlan.active())
networks = wlan.scan()
print("Wi-Fi scan results:", len(networks))
del networks
gc.collect()
ble = bluetooth.BLE()
ble.active(True)
print("BLE active:", ble.active())
counts = [0, 0, False]


def callback(event, data):
    if event == 5:
        counts[0] += 1
        adv = bytes(data[4])
        pos = 0
        while pos < len(adv):
            n = adv[pos]
            if not n or pos + n >= len(adv):
                break
            if adv[pos + 1] in (8, 9):
                name = adv[pos + 2 : pos + n + 1]
                if b"WHOOP" in name or b"Whoop" in name or b"4C0639073" in name:
                    counts[1] += 1
            pos += n + 1
    elif event == 6:
        counts[2] = True


ble.irq(callback)
ble.gap_scan(6000, 30000, 30000, False)
time.sleep_ms(7000)
print(
    "BLE scan reports:",
    counts[0],
    "WHOOP reports:",
    counts[1],
    "scan completed:",
    counts[2],
)
print("Free heap after radios:", gc.mem_free())
ble.active(False)
wlan.active(False)
