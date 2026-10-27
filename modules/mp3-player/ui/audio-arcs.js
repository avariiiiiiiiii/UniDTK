// ─── Audio visualization engine: reactive bass / mid / treble arcs ────────────
// This is NOT a widget: it drives the three concentric arcs that live in the
// mp3-player card (`.music-audio-arcs > .ac-arc`). The engine was merged out of
// the former standalone audio visualizer widget module, whose card render
// function and `.ac-container` scaffolding are gone — only the engine remains.
import { invoke } from '@unidtk/bridge';
import { lastBands, lastBandsAt } from '../../unidtk-core.js';

const AUDIO_CIRCLE_STROKE = 6;
const AUDIO_CIRCLE_PADDING = 14;
const AUDIO_CIRCLE_FRAME_MS = 33;
const AUDIO_CIRCLE_IDLE_MS = 750;
const AUDIO_BANDS_FRESH_MS = 280;
const AUDIO_ARC_ATTACK = 0.58;
const AUDIO_ARC_RELEASE = 0.2;
const AUDIO_ARC_BREATHE = 0.025;
const audioArcState = new WeakMap();
// Три дуги по 100° с промежутками по 20° между ними (всего 3×100 + 3×20 = 360°).
const AUDIO_CIRCLE_ARCS = [
  { key: "bass", start: -80, end: 20 },
  { key: "mid", start: 40, end: 140 },
  { key: "treble", start: 160, end: 260 },
];

// Рантайм-состояние дуг/аудио-заглушки (в старом renderer.js жило на верхнем уровне).
let audioCircleRaf = null;
let audioCircleTimer = null;
let audioCircleLastFrame = 0;
let audioCircleWasActive = false;
let audioStubPlaying = false;
let audioStubLastCheck = 0;
let audioStubPhaseStart = performance.now();

function setupMusicAudioArcs(card) {
  // Дуги в плеере живут чуть снаружи ring-progress (r=48), отдельный концентрический слой.
  setAudioArcPaths(card, 50, 50, 51);
}

function setAudioArcPaths(card, cx, cy, r) {
  AUDIO_CIRCLE_ARCS.forEach(({ key, start, end }) => {
    const arc = card.querySelector(`.ac-arc.${key}`);
    if (!arc) return;
    arc.setAttribute("d", audioCircleArc(cx, cy, r, start, end));
    // Scale must pivot on the SVG viewBox center (circle origin). CSS
    // transform-box:fill-box uses each arc's tight bbox and drifts the pulse.
    arc.style.transformBox = "view-box";
    arc.style.transformOrigin = "center";
  });
}

function audioCirclePoint(cx, cy, r, angle) {
  const rad = (angle - 90) * Math.PI / 180;
  return {
    x: cx + r * Math.cos(rad),
    y: cy + r * Math.sin(rad),
  };
}

function audioCircleArc(cx, cy, r, startAngle, endAngle) {
  const start = audioCirclePoint(cx, cy, r, endAngle);
  const end = audioCirclePoint(cx, cy, r, startAngle);
  const largeArc = Math.abs(endAngle - startAngle) <= 180 ? 0 : 1;
  return `M ${start.x} ${start.y} A ${r} ${r} 0 ${largeArc} 0 ${end.x} ${end.y}`;
}

function startAudioCircleLoop() {
  if (audioCircleRaf || audioCircleTimer) return;

  const tick = () => {
    audioCircleRaf = null;
    const cards = document.querySelectorAll(".music-widget");
    // Stop when no consumers remain (widget closed / canvas rebuilt empty).
    // loadWidgets() calls startAudioCircleLoop() again when arcs return.
    if (!cards.length) {
      audioCircleWasActive = false;
      audioCircleLastFrame = 0;
      return;
    }

    // The player card is never `expanded` (it has its own touch zones instead of
    // the generic click-to-expand), so the class must not gate the arcs: the
    // player would stay silent forever. Visibility = mounted and has a box.
    const visibleCards = Array.from(cards).filter((card) =>
      card.isConnected && card.getClientRects().length > 0
    );
    cards.forEach((card) => {
      if (!visibleCards.includes(card)) releaseAudioArcs(card);
    });
    if (!visibleCards.length) {
      audioCircleWasActive = false;
      audioCircleLastFrame = 0;
      audioCircleTimer = setTimeout(() => {
        audioCircleTimer = null;
        audioCircleRaf = requestAnimationFrame(tick);
      }, AUDIO_CIRCLE_IDLE_MS);
      return;
    }

    const bands = getAudioBands();
    const playing = isMusicPlayingForAudioStub();
    const active = !document.hidden && (
      Math.max(bands.bass, bands.mid, bands.treble) > 0.012
      || playing
    );

    const now = performance.now();
    const elapsed = audioCircleLastFrame ? now - audioCircleLastFrame : AUDIO_CIRCLE_FRAME_MS;
    let energy = 0;
    if (elapsed >= AUDIO_CIRCLE_FRAME_MS - 1) {
      const deltaMs = Math.min(100, elapsed);
      visibleCards.forEach((card) => {
        energy = Math.max(energy, applyAudioCircleBands(card, bands, active, deltaMs, now));
      });
      audioCircleLastFrame = now;
    }

    const wasActive = audioCircleWasActive;
    audioCircleWasActive = active;
    if (active || energy > 0.008 || (wasActive && elapsed < AUDIO_CIRCLE_FRAME_MS - 1)) {
      // Keep animation frame scheduling aligned to the browser's paint cycle;
      // a timer followed by RAF added an extra, variable frame of latency.
      audioCircleRaf = requestAnimationFrame(tick);
    } else {
      // Idle: next wake must start from a fresh frame, otherwise the first
      // delta after a long pause is huge and the arcs snap instead of rising.
      audioCircleLastFrame = 0;
      audioCircleTimer = setTimeout(() => {
        audioCircleTimer = null;
        audioCircleRaf = requestAnimationFrame(tick);
      }, AUDIO_CIRCLE_IDLE_MS);
    }
  };

  audioCircleRaf = requestAnimationFrame(tick);
}

function releaseAudioArcs(card) {
  card.querySelectorAll(".ac-arc").forEach((arc) => {
    audioArcState.delete(arc);
    arc.style.opacity = "";
    arc.style.transform = "";
    arc.style.willChange = "";
  });
}

function wakeAudioCircleLoop() {
  if (audioCircleTimer) {
    clearTimeout(audioCircleTimer);
    audioCircleTimer = null;
  }
  startAudioCircleLoop();
}

// Дуги мигают в такт: backend даёт flux+AGC, здесь — быстрый display envelope
// + glow/толщина. Цель — читаемые «удары», а не вялое дыхание.
function applyAudioCircleBands(card, bands, active = true, deltaMs = AUDIO_CIRCLE_FRAME_MS, now = performance.now()) {
  const breathe = now / 1000;
  const peak = Math.max(bands.bass || 0, bands.mid || 0, bands.treble || 0);
  const timeScale = Math.max(0.1, Math.min(3, deltaMs / AUDIO_CIRCLE_FRAME_MS));
  let maxEnergy = 0;
  ["bass", "mid", "treble"].forEach((key, i) => {
    const arc = card.querySelector(`.ac-arc.${key}`);
    if (!arc) return;

    const target = Math.max(0, Math.min(1, bands[key] || 0));
    let cur = audioArcState.get(arc);
    if (cur === undefined) cur = active ? target : 0;
    if (active) {
      const coef = target > cur ? AUDIO_ARC_ATTACK : AUDIO_ARC_RELEASE;
      const alpha = 1 - Math.pow(1 - coef, timeScale);
      cur += (target - cur) * alpha;
    } else {
      cur *= Math.pow(0.75, timeScale);
      if (cur < 0.01) cur = 0;
    }
    audioArcState.set(arc, cur);
    maxEnergy = Math.max(maxEnergy, cur);
    if (!active && cur === 0) {
      arc.style.opacity = "";
      arc.style.transform = "";
      arc.style.willChange = "";
      return;
    }

    // Breathe only when the mix is quiet — never drown real beats.
    const idle = active && peak < 0.18
      ? AUDIO_ARC_BREATHE * (0.5 + 0.5 * Math.sin(breathe * 1.8 + i * 2.1))
      : 0;
    const v = Math.min(1, cur + idle);

    const scale = 1 + v * 0.09;
    const opacity = 0.14 + v * 0.86;

    arc.style.opacity = opacity.toFixed(3);
    arc.style.transform = `scale(${scale.toFixed(4)})`;
    arc.style.willChange = active || cur > 0.01 ? "transform, opacity" : "";
  });
  return maxEnergy;
}

function normalizeBands(payload) {
  const clamp = (v) => Math.max(0, Math.min(1, Number(v) || 0));
  return {
    bass: clamp(payload?.bass),
    mid: clamp(payload?.mid),
    treble: clamp(payload?.treble),
  };
}

function getAudioBands() {
  const age = lastBands ? performance.now() - lastBandsAt : Infinity;
  if (lastBands && age < AUDIO_BANDS_FRESH_MS) return lastBands;
  // Brief hold: decay last real sample instead of jumping to fake sine.
  if (lastBands && age < 900) {
    const k = Math.max(0, 1 - (age - AUDIO_BANDS_FRESH_MS) / 620);
    return {
      bass: lastBands.bass * k,
      mid: lastBands.mid * k,
      treble: lastBands.treble * k,
    };
  }
  return getFallbackAudioBands();
}

function getFallbackAudioBands() {
  if (!isMusicPlayingForAudioStub()) {
    return { bass: 0, mid: 0, treble: 0 };
  }

  const t = (performance.now() - audioStubPhaseStart) / 1000;
  const noise = () => (Math.random() - 0.5) * 0.05;
  // Fallback only when loopback is dead: sharper kicks so arcs still feel alive.
  const beat = (rate, sharp) => Math.pow(Math.max(0, Math.sin(t * rate)), sharp);
  return normalizeBands({
    bass: 0.12 + beat(Math.PI * 4.0, 8) * 0.88 + noise(),
    mid: 0.1 + beat(Math.PI * 6.4 + 0.7, 4) * 0.65 + Math.sin(t * 5.2) * 0.1 + noise(),
    treble: 0.08 + beat(Math.PI * 9.4 + 1.4, 3) * 0.55 + Math.sin(t * 8.4) * 0.12 + noise(),
  });
}

function isMusicPlayingForAudioStub() {
  const localPlaying = [...document.querySelectorAll(".music-widget")]
    .some((card) => card._isPlaying === true);
  if (localPlaying) {
    audioStubPlaying = true;
    return true;
  }

  const now = performance.now();
  if (now - audioStubLastCheck > 2000) {
    audioStubLastCheck = now;
    invoke("player_status")
      .then((data) => {
        if (data) audioStubPlaying = !!data.running && !data.paused;
      })
      .catch(() => {
        audioStubPlaying = false;
      });
  }

  return audioStubPlaying;
}

export { setupMusicAudioArcs, startAudioCircleLoop, wakeAudioCircleLoop, normalizeBands, getAudioBands, isMusicPlayingForAudioStub };
