#!/usr/bin/env python3
"""Synthesize Mac Bot notification sounds (original, no license needed).

Each sound is two short notes built from sine partials with a fast attack and
exponential decay. Output: design/sounds/*.wav (44.1 kHz, 16-bit, mono).
"""
import math, pathlib, struct, wave

SR = 44100
OUT = pathlib.Path(__file__).resolve().parent.parent / "sounds"

def note(freq, dur, partials, decay, attack=0.004, vib=0.0):
    n = int(SR * dur)
    out = []
    for i in range(n):
        t = i / SR
        env = min(1.0, t / attack) * math.exp(-t / decay)
        f = freq * (1 + vib * math.sin(2 * math.pi * 6 * t))
        s = sum(a * math.sin(2 * math.pi * f * m * t) * math.exp(-t * k) for m, a, k in partials)
        out.append(env * s)
    return out

def mix(*parts):
    """parts: (start_seconds, samples)"""
    length = max(int(st * SR) + len(sm) for st, sm in parts)
    buf = [0.0] * length
    for st, sm in parts:
        o = int(st * SR)
        for i, v in enumerate(sm):
            buf[o + i] += v
    return buf

def save(name, buf, peak=0.45):
    m = max(abs(v) for v in buf) or 1
    fade = int(SR * 0.01)
    for i in range(fade):  # short fade-out to avoid clicks
        buf[-1 - i] *= i / fade
    with wave.open(str(OUT / f"{name}.wav"), "w") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes(b"".join(struct.pack("<h", int(v / m * peak * 32767)) for v in buf))
    print(f"{name}.wav  {len(buf) / SR * 1000:.0f} ms")

# timbres: (multiple, amplitude, extra damping per second)
GLASS   = [(1, 1.0, 0), (2.76, 0.25, 18), (5.4, 0.08, 40)]
MARIMBA = [(1, 1.0, 0), (4.0, 0.30, 35), (10.0, 0.05, 80)]
KALIMBA = [(1, 1.0, 0), (3.0, 0.18, 25), (5.9, 0.07, 60)]
SOFT    = [(1, 1.0, 0), (2.0, 0.12, 20)]
WOOD    = [(1, 1.0, 0), (2.4, 0.35, 60), (4.1, 0.15, 90)]

def hz(semitones_from_a4):
    return 440 * 2 ** (semitones_from_a4 / 12)

C6, D6, E6, G6, A5, B5, E5, G5, Fs5, Cs6, A6 = (hz(x) for x in (15, 17, 19, 22, 12, 14, 7, 10, 9, 16, 24))

# 1. 叮咚 · 玻璃（上行纯五度）：通用新消息
save("01-glass-ding-dong", mix((0, note(E6, .35, GLASS, .12)), (.085, note(B5 * 2, .45, GLASS, .16))))
# 2. 嘀嗒 · 马林巴（上行大三度）：轻、木质
save("02-marimba-up", mix((0, note(C6, .22, MARIMBA, .07)), (.07, note(E6, .32, MARIMBA, .1))))
# 3. 卡林巴（上行四度）：温暖、有特点
save("03-kalimba-pluck", mix((0, note(A5, .28, KALIMBA, .09)), (.09, note(D6, .42, KALIMBA, .14))))
# 4. 啵啵 · 气泡（极短）：私聊里的轻提示
save("04-bubble-pop", mix((0, note(G5 * 2, .07, SOFT, .025)), (.06, note(C6 * 2, .09, SOFT, .03))))
# 5. 木鱼 · 嗒嗒：任务完成
save("05-wood-tock", mix((0, note(G5, .12, WOOD, .035)), (.075, note(C6, .16, WOOD, .045))))
# 6. 需要你 · 明亮（上行再上行，略醒目）：审批 / 提问 / 接管
save("06-attention", mix((0, note(Fs5 * 2, .25, GLASS, .08)), (.1, note(Cs6 * 2, .25, GLASS, .08)), (.2, note(A6, .5, GLASS, .18))), peak=.5)
# 7. 完成 · 下行柔和：待验收
save("07-review-soft", mix((0, note(G6, .3, SOFT, .1)), (.11, note(C6, .5, SOFT, .2))))
