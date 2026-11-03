// TSTV Phone Mics — desktop UI. Talks to the Rust side through Tauri commands and a
// 20 Hz "tick" event carrying meters and status.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const METER_FLOOR_DB = -60;
const SCALE_MARKS = [0, -6, -12, -18, -24, -36, -48, -60];
const PEAK_HOLD_MS = 1500;
const FALL_DB_PER_S = 24;

const STATE_TEXT = {
  empty: ["Waiting for a phone", ""],
  waiting: ["Connected – no audio yet", "warn"],
  buffering: ["Buffering…", "warn"],
  live: ["Live", "ok"],
  "no-audio": ["No audio – check the phone", "bad"],
};

const $ = (sel, root = document) => root.querySelector(sel);
let setup = null;
let strips = [];
let deviceChannels = 0;

function meterPos(db) {
  return Math.min(1, Math.max(0, (db - METER_FLOOR_DB) / -METER_FLOOR_DB));
}

function fmtDb(db) {
  return db <= -60 ? "−∞ dB" : `${db > 0 ? "+" : ""}${db.toFixed(1)} dB`;
}

function fmtMs(v, digits = 0) {
  return v == null ? "—" : `${v.toFixed(digits)} ms`;
}

async function init() {
  setup = await invoke("get_setup");

  const ips = setup.ips.length ? setup.ips.map((ip) => `<b>${ip}</b>`).join(" or ") : "<b>no network found</b>";
  $("#connect-info").innerHTML = `Phones connect to ${ips} · port ${setup.controlPort} · or pick “${setup.desktopName}”`;
  $("#footer-note").textContent = `v${setup.version} · Milestone 1 (${setup.activeSlots} phone)`;
  $("#log-path").textContent = `Log: ${setup.logPath}`;

  fillDevices();
  $("#device").addEventListener("mousedown", refreshDevices);
  $("#device").addEventListener("change", (e) => invoke("set_device", { device: e.target.value || null }));

  const buffer = $("#buffer");
  buffer.min = setup.minBufferMs;
  buffer.max = setup.maxBufferMs;
  buffer.value = setup.bufferMs;
  $("#buffer-val").textContent = `${setup.bufferMs} ms`;
  buffer.addEventListener("input", () => ($("#buffer-val").textContent = `${buffer.value} ms`));
  buffer.addEventListener("change", () => invoke("set_buffer_ms", { ms: Number(buffer.value) }));

  let allMuted = false;
  $("#mute-all").addEventListener("click", () => {
    allMuted = !strips.every((s) => s.operatorMuted);
    invoke("mute_all", { muted: allMuted });
  });

  const main = $("#strips");
  for (let i = 0; i < setup.activeSlots; i++) {
    strips.push(makeStrip(i, setup.slots[i]));
    main.appendChild(strips[i].el);
  }

  await listen("tick", (e) => update(e.payload));
  requestAnimationFrame(animateMeters);
}

function fillDevices() {
  const sel = $("#device");
  sel.innerHTML = "";
  sel.add(new Option("Automatic (StudioLive if connected)", ""));
  for (const d of setup.devices) {
    sel.add(new Option(`${d.name} — ${d.channels} ch`, d.name));
  }
  if (setup.selectedDevice && !setup.devices.some((d) => d.name === setup.selectedDevice)) {
    sel.add(new Option(`${setup.selectedDevice} (not connected)`, setup.selectedDevice));
  }
  sel.value = setup.selectedDevice ?? "";
}

async function refreshDevices() {
  const fresh = await invoke("get_setup");
  setup.devices = fresh.devices;
  setup.selectedDevice = fresh.selectedDevice;
  fillDevices();
}

function makeStrip(slot, saved) {
  const el = $("#strip-template").content.firstElementChild.cloneNode(true);
  const s = {
    el, slot,
    operatorMuted: false,
    tone: false,
    shownDb: METER_FLOOR_DB,
    peakDb: METER_FLOOR_DB,
    peakAt: 0,
    lastTick: performance.now(),
    channelOptions: 0,
  };
  $(".slot-label", el).textContent = `PHONE ${slot + 1}`;

  const scale = $(".scale", el);
  for (const db of SCALE_MARKS) {
    const mark = document.createElement("span");
    mark.textContent = db;
    mark.style.bottom = `${meterPos(db) * 100}%`;
    scale.appendChild(mark);
  }

  const channel = $(".channel", el);
  fillChannels(s, 64, saved.channel);
  channel.addEventListener("change", () => invoke("set_channel", { slot, channel: Number(channel.value) }));

  const fader = $(".fader", el);
  fader.value = saved.gainDb;
  $(".gain", el).textContent = fmtDb(saved.gainDb);
  const sendGain = () => {
    $(".gain", el).textContent = fmtDb(Number(fader.value));
    invoke("set_gain", { slot, db: Number(fader.value) });
  };
  fader.addEventListener("input", sendGain);
  fader.addEventListener("dblclick", () => { fader.value = 0; sendGain(); });

  $(".mute", el).addEventListener("click", () => invoke("set_mute", { slot, muted: !s.operatorMuted }));
  $(".tone", el).addEventListener("click", () => invoke("set_tone", { slot, on: !s.tone }));
  return s;
}

function fillChannels(s, count, selected) {
  const sel = $(".channel", s.el);
  if (s.channelOptions === count && sel.value == selected) return;
  sel.innerHTML = "";
  for (let ch = 1; ch <= Math.max(count, selected); ch++) {
    sel.add(new Option(`Ch ${ch}`, ch));
  }
  sel.value = selected;
  s.channelOptions = count;
}

function update(tick) {
  const eng = tick.engine;
  const status = $("#engine-status");
  if (eng.running) {
    const khz = (eng.sampleRate / 1000).toFixed(eng.sampleRate % 1000 ? 1 : 0);
    status.textContent = `Output: ${eng.device} · ${eng.channels} ch · ${khz} kHz · ${eng.outputLatencyMs.toFixed(1)} ms`;
    status.className = eng.sampleRate === 48000 ? "pill ok" : "pill warn";
    if (eng.sampleRate !== 48000) status.textContent += " — set the mixer to 48 kHz";
  } else {
    status.textContent = eng.error ? `Output not open: ${eng.error}` : "Opening audio output…";
    status.className = "pill bad";
  }
  deviceChannels = eng.running ? eng.channels : 0;

  const banner = $("#banner");
  banner.hidden = !tick.networkError;
  banner.textContent = tick.networkError ?? "";

  for (const s of strips) {
    const v = tick.slots[s.slot];
    if (!v) continue;
    const el = s.el;
    s.operatorMuted = v.operatorMuted;
    s.tone = v.tone;

    const [text, cls] = STATE_TEXT[v.state] ?? [v.state, ""];
    const state = $(".state", el);
    state.textContent = text;
    state.className = `state pill ${cls}`;
    el.classList.toggle("live", v.state === "live" && !v.operatorMuted);
    el.classList.toggle("alert", v.state === "no-audio" || v.interrupted);

    $(".name", el).textContent = v.connected ? v.name : "—";
    $(".model", el).textContent = v.connected ? v.model : "";
    $(".mic", el).textContent = v.mic ? `Mic: ${v.mic}${v.micKind === "bluetooth" ? " (Bluetooth)" : ""}` : "";

    $(".flag-talent", el).hidden = !v.talentMuted;
    $(".flag-interrupt", el).hidden = !v.interrupted;
    const badChannel = deviceChannels > 0 && v.channel > deviceChannels;
    $(".flag-channel", el).hidden = !badChannel;
    $(".flag-channel", el).textContent = `This device has only ${deviceChannels} outputs`;

    const mute = $(".mute", el);
    mute.classList.toggle("on", v.operatorMuted);
    mute.textContent = v.operatorMuted ? "Muted" : "Mute";
    const tone = $(".tone", el);
    tone.classList.toggle("on", v.tone);
    tone.textContent = v.tone ? "Tone on" : "Test tone";

    if (deviceChannels) fillChannels(s, deviceChannels, v.channel);

    // Meter: rise instantly, fall smoothly (animateMeters does the falling).
    if (v.levelDb > s.shownDb) s.shownDb = v.levelDb;
    if (v.levelDb >= s.peakDb || performance.now() - s.peakAt > PEAK_HOLD_MS) {
      s.peakDb = v.levelDb;
      s.peakAt = performance.now();
    }
    $(".level", el).textContent = v.levelDb <= -119 ? "−∞" : `${v.levelDb.toFixed(1)} dBFS`;

    const lat = $(".latency", el);
    lat.textContent = fmtMs(v.latencyMs);
    lat.className = "latency mono" + (v.latencyMs == null ? "" : v.latencyMs < 100 ? " good" : v.latencyMs < 150 ? " meh" : " bad");
    $(".buf", el).textContent = v.connected ? fmtMs(v.bufferMs) : "—";
    $(".loss", el).textContent = v.connected ? `${v.lossPct.toFixed(1)} %` : "—";
    $(".loss", el).title = `Late packets: ${v.late}, rebuilt from backup copy: ${v.recovered}, concealed: ${v.concealed}`;
    $(".jitter", el).textContent = v.connected ? fmtMs(v.jitterMs, 1) : "—";
    $(".rtt", el).textContent = v.connected ? fmtMs(v.rttMs, 1) : "—";
    $(".battery", el).textContent = v.battery == null ? "—" : `${Math.round(v.battery * 100)} %`;
  }
}

function animateMeters(now) {
  for (const s of strips) {
    const dt = (now - s.lastTick) / 1000;
    s.lastTick = now;
    s.shownDb = Math.max(METER_FLOOR_DB, s.shownDb - FALL_DB_PER_S * dt);
    $(".meter-cover", s.el).style.height = `${(1 - meterPos(s.shownDb)) * 100}%`;
    $(".meter-peak", s.el).style.bottom = `calc(${meterPos(s.peakDb) * 100}% - 2px)`;
  }
  requestAnimationFrame(animateMeters);
}

init().catch((e) => {
  document.body.innerHTML = `<pre style="padding:20px;color:#e5484d">Startup error: ${e}</pre>`;
});
