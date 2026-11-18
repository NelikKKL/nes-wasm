# nes-wasm

NES-эмулятор на Rust, собранный в WebAssembly. Подключается к любой странице как библиотека:
окно игры, клавиатура и геймпад с переназначением клавиш, звук и главный цикл уже внутри.

## Три файла

| Файл | Что это |
|---|---|
| `nes_wasm_bg.wasm` | ядро эмулятора |
| `nes_wasm.js` | JS-обвязка, сгенерированная из Rust (в ней же весь API) |
| `audio-worklet.js` | звуковой обработчик (AudioWorklet) |

Положи их рядом на своём сайте. Готовые файлы собирает GitHub Action: артефакт **nes-wasm-lib**
в запуске workflow.

## Быстрый старт

```html
<div id="game"></div>
<script type="module">
  import init, { Nes } from './nes_wasm.js';

  await init();
  const nes = new Nes(document.getElementById('game'));   // внутри создаётся canvas
  await nes.loadRomFromUrl('game.nes');                    // или nes.loadRom(uint8array)
</script>
```

Звук включится на первом нажатии/клике (так требует браузер). Играть можно сразу:
клавиши по умолчанию: стрелки, `X` (A), `Z` (B), `Enter` (Start), правый `Shift` (Select);
второй игрок: `WASD`, `G` (A), `F` (B), `T` (Start), `R` (Select).

## Опции конструктора

`new Nes(target, options)`

`target` — `<canvas>` (используется как есть), любой контейнер (canvas создаётся внутри) или `null`
(забери canvas через `nes.canvas` и поставь куда нужно).

| Опция | По умолчанию | Смысл |
|---|---|---|
| `scale` | на всю ширину контейнера | размер canvas: 256·scale пикселей |
| `keyboard` | `true` | управлять с клавиатуры |
| `gamepad` | `true` | управлять с геймпада |
| `autoAudio` | `true` | включать звук на первом жесте пользователя |
| `muted` | `false` | стартовать без звука |
| `volume` | `1.5` | громкость |
| `storageKey` | `"nes-bindings"` | ключ в localStorage для привязок клавиш |
| `workletUrl` | `"audio-worklet.js"` | где лежит `audio-worklet.js` (путь считается от страницы) |

Если файлы лежат не рядом со страницей:
`workletUrl: new URL('./audio-worklet.js', import.meta.url).href`.

## Методы и свойства

| | |
|---|---|
| `loadRom(bytes)` | загрузить ROM (`Uint8Array`); при ошибке бросает строку |
| `loadRomFromUrl(url)` | скачать и загрузить; возвращает Promise |
| `hasRom` | загружена ли игра |
| `pause()` / `resume()` / `paused` | пауза |
| `reset()` | сброс консоли |
| `fullscreen()` | во весь экран |
| `canvas` | canvas игры |
| `enableAudio()` | включить звук вручную (при `autoAudio: false`, из обработчика клика) |
| `setMuted(bool)` / `muted` | выключить звук |
| `setVolume(x)` / `volume` | громкость |
| `audioStatus()` | строка про состояние звука для диагностики |
| `getBindings()` | текущие клавиши: `[ {A:'KeyX', ...}, {...} ]` для игроков 1 и 2 |
| `setBinding(player, button, code)` | назначить клавишу (`KeyboardEvent.code`) |
| `captureBinding(player, button)` | Promise: ждёт нажатия клавиши и назначает её |
| `cancelCapture()` | отменить ожидание клавиши |
| `resetBindings()` | вернуть клавиши по умолчанию |
| `destroy()` | остановить цикл, снять обработчики, закрыть звук |

`player`: `0` или `1`. `button`: `A`, `B`, `Select`, `Start`, `Up`, `Down`, `Left`, `Right`.
Привязки сохраняются в localStorage.

### Переназначение клавиши

```js
const code = await nes.captureBinding(0, 'A');   // следующая нажатая клавиша станет кнопкой A
// Esc отменяет (вернётся ''), Backspace снимает привязку
console.log(nes.getBindings()[0].A);
```

Готовое окно настроек на этом API смотри в `examples/index.html`.

## Звук

Звук идёт через AudioWorklet. Браузер даёт его только на **HTTPS или `localhost`**. На обычном HTTP
библиотека автоматически переключается на запасной вариант (`ScriptProcessorNode`), и `audioStatus()`
пишет «режим совместимости».

## Низкоуровневый API

Если нужен только процессор без окна и звука, есть класс `Emulator`:
`new Emulator(rom)`, `frame()`, `frame_ptr()`, `set_pad(player, state)`, `audio_ptr()`, `audio_len()`,
`set_sample_rate(rate)`, `reset()`.

## Что умеет эмулятор

- CPU 6502 (все официальные опкоды), PPU с фоном, спрайтами и скроллингом
- Мапперы: 0 (NROM), 1 (MMC1), 2 (UxROM), 3 (CNROM), 4 (MMC3), 7 (AxROM)
- APU: 2 pulse, triangle, noise, DMC, frame counter с IRQ
- Два игрока

## Сборка

```sh
cargo install wasm-pack
wasm-pack build --release --target web --out-dir pkg
```

Файлы библиотеки: `pkg/nes_wasm.js`, `pkg/nes_wasm_bg.wasm` и `js/audio-worklet.js`.
Демо: положи их вместе с `examples/index.html` в одну папку и открой через локальный сервер
(`python3 -m http.server`): ES-модули не работают с `file://`.

## GitHub Pages

Settings → Pages → Source: **GitHub Actions**. Каждый push в `main` собирает библиотеку и публикует
демо вместе с тремя файлами.
