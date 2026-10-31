import bluetooth, time, struct, json

ble = bluetooth.BLE()
ble.active(True)
state = {
    "target": None,
    "connection": None,
    "services": None,
    "hr": None,
    "cccd": None,
    "samples": 0,
    "error": None,
    "action": None,
    "notifications": [],
}
HR_SERVICE = bluetooth.UUID(0x180D)
HR_VALUE = bluetooth.UUID(0x2A37)
CCCD = bluetooth.UUID(0x2902)


def callback(event, data):
    if event == 5:
        adv = bytes(data[4])
        pos = 0
        while pos < len(adv):
            n = adv[pos]
            if not n or pos + n >= len(adv):
                break
            if adv[pos + 1] in (8, 9) and b"4C0639073" in adv[pos + 2 : pos + n + 1]:
                state["target"] = (data[0], bytes(data[1]))
            pos += n + 1
    elif event == 7:
        state["connection"] = data[0]
        print("Pico connected to target WHOOP")
        state["action"] = "services"
    elif event == 9:
        if data[3] == HR_SERVICE:
            state["services"] = (data[1], data[2])
    elif event == 10:
        if state["services"]:
            state["action"] = "characteristics"
        else:
            state["error"] = "HR service missing"
    elif event == 11:
        if data[4] == HR_VALUE:
            state["hr"] = data[2]
    elif event == 12:
        if state["hr"]:
            # This WHOOP exposes one HR characteristic followed by one CCCD.
            # Reject other layouts instead of guessing descriptor handles.
            if state["services"][1] != state["hr"] + 1:
                state["error"] = "Unexpected HR descriptor layout"
            else:
                state["cccd"] = state["hr"] + 1
                state["action"] = "subscribe"
        else:
            state["error"] = "HR characteristic missing"
    elif event == 17:
        print("CCCD write status:", data[2])
        if data[2]:
            state["error"] = "CCCD write rejected"
    elif event == 18:
        if data[1] != state["hr"]:
            return
        # IRQ callbacks only copy notifications; parse/print in the main loop.
        if len(state["notifications"]) >= 32:
            state["error"] = "Notification queue overflow"
        else:
            state["notifications"].append(bytes(data[2]))


def consume():
    while state["notifications"]:
        raw = state["notifications"].pop(0)
        pos = 1
        if raw[0] & 1:
            bpm = struct.unpack_from("<H", raw, pos)[0]
            pos += 2
        else:
            bpm = raw[pos]
            pos += 1
        energy = None
        if raw[0] & 8:
            energy = struct.unpack_from("<H", raw, pos)[0]
            pos += 2
        rr = []
        if raw[0] & 16:
            while pos + 1 < len(raw):
                rr.append(round(struct.unpack_from("<H", raw, pos)[0] * 1000 / 1024))
                pos += 2
        state["samples"] += 1
        print(
            json.dumps(
                {
                    "source": "pico_ble_diagnostic",
                    "bpm": bpm,
                    "rr_ms": rr,
                    "raw_bytes": list(raw),
                    "gatt": {
                        "flags": raw[0],
                        "energy_kj": energy,
                        "contact_supported": bool(raw[0] & 4),
                        "skin_contact": bool(raw[0] & 2) if raw[0] & 4 else None,
                    },
                }
            )
        )


ble.irq(callback)
try:
    ble.gap_scan(8000, 30000, 30000, False)
    deadline = time.ticks_add(time.ticks_ms(), 10000)
    while state["target"] is None and time.ticks_diff(deadline, time.ticks_ms()) > 0:
        time.sleep_ms(100)
    if state["target"] is None:
        raise RuntimeError("Target WHOOP advertisement not found")
    ble.gap_scan(None)
    time.sleep_ms(200)
    ble.gap_connect(*state["target"])
    deadline = time.ticks_add(time.ticks_ms(), 25000)
    while (
        state["samples"] < 5
        and state["error"] is None
        and time.ticks_diff(deadline, time.ticks_ms()) > 0
    ):
        action = state["action"]
        state["action"] = None
        if action:
            print("GATT operation:", action)
        if action == "services":
            ble.gattc_discover_services(state["connection"], HR_SERVICE)
        elif action == "characteristics":
            ble.gattc_discover_characteristics(state["connection"], *state["services"])
        elif action == "subscribe":
            ble.gattc_write(state["connection"], state["cccd"], b"\x01\x00", 1)
        consume()
        time.sleep_ms(100)
    if state["error"]:
        raise RuntimeError(state["error"])
    if state["samples"] < 5:
        print(
            "Final GATT state:",
            {k: v for k, v in state.items() if k not in ("target", "notifications")},
        )
        raise RuntimeError("Not enough WHOOP measurements")
    print("PASS: Pico BLE connection and five live HR/RR notifications")
finally:
    if state["connection"] is not None:
        ble.gap_disconnect(state["connection"])
        time.sleep_ms(500)
    ble.active(False)
