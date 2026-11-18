# nes-wasm

NES-эмулятор на Rust, собирается в WebAssembly и работает в браузере.

## Что умеет

- CPU 6502 (все официальные опкоды), PPU с фоном, спрайтами и скроллингом
- Мапперы: 0 (NROM), 1 (MMC1), 2 (UxROM), 3 (CNROM), 7 (AxROM)
- Два игрока, клавиатура и геймпад
- Свои бинды клавиш (кнопка «Управление»), сохраняются в localStorage
- Загрузка ROM через выбор файла или drag-and-drop

Звука пока нет, мапперы MMC3 и выше тоже.

## Деплой на GitHub Pages

1. Залей репозиторий на GitHub (ветка `main`).
2. Settings → Pages → Source: **GitHub Actions**.
3. Каждый push в `main` собирает wasm через `wasm-pack` и публикует папку `web/`.

## Локально

```sh
cargo install wasm-pack
wasm-pack build --release --target web --out-dir web/pkg
cd web && python3 -m http.server 8080
```

Открой http://localhost:8080 (ES-модули не работают с `file://`).

## Управление по умолчанию

| | Игрок 1 | Игрок 2 |
|---|---|---|
| D-pad | стрелки | WASD |
| A / B | X / Z | G / F |
| Select / Start | R Shift / Enter | R / T |

Геймпад: крестовина или левый стик, A/B — правые кнопки, Select/Start — как обычно.
