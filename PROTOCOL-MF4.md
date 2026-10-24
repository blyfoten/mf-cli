# MiniFuse 4 USB control protocol

Reverse-engineered from USB captures of MiniFuse Control Center 1.4.2 (Windows) talking to a
MiniFuse 4 (VID `0x1c75`, PID `0xaf70`). Everything below was observed on the wire unless marked
*unconfirmed*.

## Transport

All requests go to the control endpoint, recipient = interface 0, `wIndex = 0`.

| Direction | bmRequestType | bRequest | wValue | wLength | Purpose |
|---|---|---|---|---|---|
| write | `0x21` | `0x22` | `(selector << 8) \| channel` | 2 | set one parameter |
| read  | `0xa1` | `0xa2` | `(selector << 8) \| channel` | 2 | read one parameter |
| read  | `0xa1` | `0xb2` | `0` | 190 | read the whole settings block |
| read  | `0xa1` | `0xc2` | `0` | 64 | level meters |

This is the same write request mf-cli already uses for the MiniFuse 1/2.

- Write data is a little-endian 16-bit value.
- For **bitmask** parameters (INST, mute, solo), `channel` picks the bit and the data is `1`/`0`.
- For **array** parameters (input gain), `channel` picks the element: the value lives at selector
  `sel + channel`.
- For **plain** parameters, `channel` is `0` and the data is the value.
- The device sends no interrupt notifications. Control Center polls the settings block about 4 times
  per second, so changes made on the front panel show up there too.

## Settings block (`0xb2`, 190 bytes)

95 little-endian 16-bit values. **Value for selector `s` is at byte offset `2 * s`**, and writing
selector `s` changes that value.

| Selector | Meaning | Values | Default | Status |
|---|---|---|---|---|
| `0x00` | INST | bit 0 = input 1, bit 1 = input 2 | `0` | confirmed |
| `0x03` | 48V phantom (both inputs) | 0/1 | `0` | confirmed. **Not `0x04` as on MiniFuse 1/2** |
| `0x06` | Link input 3/4 gain | 0/1 | `0` | confirmed (on). Control Center copies input 3 gain to input 4 and then writes both; the device does not link them itself |
| `0x09`–`0x44` | Mixer gains, see below | int16, 1/256 dB | | confirmed for Out 1/2 |
| `0x48`–`0x4b` | Input gain array, written as wValue `0x48nn` (nn = input 0–3); only inputs 3/4 are used | int16, 1/256 dB, −24 … +24 dB | `0` | confirmed for inputs 3/4 |
| `0x45` | Master, Outputs 1&2 | int16, 1/256 dB, −64 dB (`0xc000`) … 0 dB | `0` | confirmed |
| `0x46` | Master, Outputs 3&4 | as above | `0` | *unconfirmed*; mf-cli writes wValue `0x4600` |
| `0x47` | Master, Loopback | as above | `0` | *unconfirmed*; mf-cli writes wValue `0x4700` |
| `0x4c` | Mute mask, Outputs 1&2 | bit per source, 1 = muted | `0xffcf` | confirmed |
| `0x4d` | Mute mask, Outputs 3&4 | bit per source, 1 = muted | `0xff3f` | confirmed |
| `0x4e` | Mute mask, Loopback | bit per source, 1 = muted | `0xffff` | *unconfirmed* |
| `0x4f` | Solo mask, Outputs 1&2 | bit per source, 1 = soloed | `0` | confirmed |
| `0x50` | Solo mask, Outputs 3&4 | bit per source | `0` | *unconfirmed* |
| `0x51` | Solo mask, Loopback | bit per source | `0` | *unconfirmed* |
| `0x52`–`0x54` | Mixer layout: mono strips shown on Out 1&2 / Out 3&4 / Loopback, written per bit like the mute mask | bit per input 1–4 | `0xf`, `0xf`, `0` | confirmed for Out 1&2 and Out 3&4 |
| `0x55`–`0x57` | Mixer layout: stereo strips shown on Out 1&2 / Out 3&4 / Loopback, written per bit | bit 0 In 1/2, 1 In 3/4, 2 USB 1/2, 3 USB 3/4, 4 USB 5/6 | `0x04`, `0x08`, `0x10` | confirmed for Out 1&2 and Out 3&4 |
| `0x58` | Outputs 1&2 source | `0` = USB1/2 direct (bypass mixer), `1` = mixer | `1` | confirmed |
| `0x59` | Outputs 3&4 source | `2` = USB3/4 direct (bypass mixer), `3` = mixer | `3` | confirmed |
| `0x5a` | Loopback source | `4` = USB5/6, `5` = Cue2Mix, `6` = OUT1/2, `7` = OUT3/4 | `4` | confirmed |
| `0x05` | ? | | `1` | unknown |
| `0x07` | ? (polled constantly by the background agent) | | `0` | unknown |
| `0x08` | ? | | `4` | unknown |
| `0x5b`–`0x5e` | ? | | | unknown |

### Sources (mixer inputs, mask bits)

| Index | Source |
|---|---|
| 0–3 | Analog 1–4 |
| 4–5 | USB 1/2 (computer playback) |
| 6–7 | USB 3/4 |
| 8–9 | USB 5/6 |

### Mixer gains (`0x09`–`0x44`)

An array per bus channel: the value lives at selector `0x09 + bus * 10 + source`, and Control
Center writes it with wValue `((0x09 + bus * 10) << 8) | source` (e.g. `0x1d02` = Analog 3 → Out 3).

| Bus | Output channel |
|---|---|
| 0, 1 | Outputs 1&2, left / right |
| 2, 3 | Outputs 3&4, left / right |
| 4, 5 | Loopback, left / right |

Each value is the final gain from that source to that output channel, a signed 16-bit value in
1/256 dB. `0x8000` (−128 dB) means off.

Control Center does not store fader and balance separately; it writes both channels of the bus with

```
left  = fader_dB + 20 * log10(cos(θ))
right = fader_dB + 20 * log10(sin(θ))     θ = 0 (full left) … π/2 (full right)
```

A centred source at 0 dB is stored as −3.01 dB on both sides (`0xfcfe`). Fader range is
+12 dB … −60 dB. By default the USB pairs are hard-panned (0 dB on their own side, −60 dB on the
other).

### Mixer layout (`0x52`–`0x57`)

The layout selectors only tell Control Center which strips to draw. When a strip is added or
removed, Control Center itself writes the related mute (removed strips are muted), solo and gain
values; a stereo pair is added hard-panned (0 dB on its own side, `0x8000` on the other).

## Level meters (`0xc2`, 64 bytes)

16 little-endian 32-bit linear peak levels, `0x7fffffff` = 0 dBFS. Found by playing into Analog 2
and muting sources in the mixer while reading the meters.

| Index | Meter |
|---|---|
| 0–3 | Analog 1–4 |
| 4–9 | USB 1–6 (computer playback) |
| 10, 11 | Outputs 1&2, left / right |
| 12, 13 | Outputs 3&4, left / right |
| 14, 15 | Loopback, left / right |

The input and USB meters are the sources before the mixer; the output meters follow the mixer (a
centred source at 0 dB reads 3 dB below its input meter). Indices 12–15 are *unconfirmed*: they
read 0 while everything on those mixers was muted.
