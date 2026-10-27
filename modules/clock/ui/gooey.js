/** Soft, stepped clock glyphs inspired by the Nothing OS Gooey clockface.
 * Paths are editable geometry: there is no font, bitmap, or filter dependency.
 * Every digit has the same advance, so minute changes cannot move the clock.
 */
const ZERO = 'M20 2H44Q50 2 50 8Q50 14 57 14Q64 14 64 21V55Q64 62 57 62Q50 62 50 68Q50 74 44 74H20Q14 74 14 68Q14 62 7 62Q0 62 0 55V21Q0 14 7 14Q14 14 14 8Q14 2 20 2Z M20 17Q14 17 14 23V53Q14 59 20 59H44Q50 59 50 53V23Q50 17 44 17Z';
const ONE = 'M31 2Q38 2 38 9V56Q38 62 45 62H47Q54 62 54 68Q54 74 47 74H17Q10 74 10 68Q10 62 17 62H19Q24 62 24 56V33Q24 29 19 29Q10 29 10 22Q10 16 17 16Q24 16 24 9Q24 2 31 2Z';
const STROKES = {
  2: 'M9 19Q9 9 20 9H44Q55 9 55 19V24Q55 34 43 36L25 40Q12 43 9 55L9 67H55',
  3: 'M9 9H44Q55 9 55 20V24Q55 38 41 38H24M41 38Q55 38 55 51V56Q55 67 44 67H9',
  4: 'M17 9L9 40Q8 46 16 46H55M46 9V67',
  5: 'M55 9H9V34H43Q55 34 55 46V55Q55 67 43 67H9',
  6: 'M53 9H23Q9 9 9 24V54Q9 67 22 67H43Q55 67 55 54V48Q55 35 43 35H9',
  7: 'M9 9H55V20Q55 29 44 33Q33 38 32 49L30 67',
  8: 'M23 9H41Q54 9 54 21V25Q54 38 41 38H23Q10 38 10 25V21Q10 9 23 9Z M23 38H41Q55 38 55 51V55Q55 67 41 67H23Q9 67 9 55V51Q9 38 23 38Z',
  9: 'M11 67H41Q55 67 55 52V22Q55 9 42 9H21Q9 9 9 22V28Q9 41 21 41H55',
};

const SVG_NS = 'http://www.w3.org/2000/svg';

function digitMarkup(digit) {
  if (digit === '0') {
    return `<path fill="currentColor" fill-rule="evenodd" d="${ZERO}"/><circle fill="currentColor" cx="32" cy="38" r="6.2"/>`;
  }
  if (digit === '1') return `<path fill="currentColor" d="${ONE}"/>`;
  return `<path d="${STROKES[digit]}" fill="none" stroke="currentColor" stroke-width="14" stroke-linecap="round" stroke-linejoin="round"/>`;
}

export function renderGooeyClock(container) {
  container.innerHTML = `<div class="gooey-clock-face" role="img">
    <time class="gooey-clock-date" aria-hidden="true"></time>
    <svg class="gooey-clock-digits" viewBox="-2 -2 278 80" aria-hidden="true" focusable="false"></svg>
  </div>`;
  const svg = container.querySelector('.gooey-clock-digits');
  for (let index = 0; index < 4; index += 1) {
    const slot = document.createElementNS(SVG_NS, 'g');
    slot.setAttribute('transform', `translate(${index * 70} 0)`);
    slot.classList.add('gooey-clock-slot');
    svg.appendChild(slot);
  }
  updateGooeyClock(container.querySelector('.gooey-clock-face'), new Date());
}

export function updateGooeyClock(face, now) {
  const hours = String(now.getHours()).padStart(2, '0');
  const minutes = String(now.getMinutes()).padStart(2, '0');
  const digits = hours + minutes;
  const dateKey = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
  const dateChanged = face.dataset.date !== dateKey;
  if (dateChanged) {
    const date = face.querySelector('.gooey-clock-date');
    date.dateTime = dateKey;
    date.textContent = now.toLocaleDateString('ru-RU', {
      weekday: 'short', day: 'numeric', month: 'short',
    }).replaceAll('.', '');
    face.dataset.date = dateKey;
  }
  if (face.dataset.time === digits && !dateChanged) return;
  const slots = face.querySelectorAll('.gooey-clock-slot');
  for (let index = 0; index < slots.length; index += 1) {
    const slot = slots[index];
    if (slot.dataset.digit === digits[index]) continue;
    // Replacing only the changed ink keeps fixed slots and the date stationary.
    slot.innerHTML = `<g class="gooey-clock-ink">${digitMarkup(digits[index])}</g>`;
    slot.dataset.digit = digits[index];
  }
  face.dataset.time = digits;
  face.setAttribute('aria-label', `${hours}:${minutes}, ${now.toLocaleDateString('ru-RU', {
    weekday: 'long', day: 'numeric', month: 'long', year: 'numeric',
  })}`);
}
