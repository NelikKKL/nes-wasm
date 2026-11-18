// Кольцевой буфер сэмплов. Основной поток шлёт сюда Float32Array из эмулятора,
// а мы в ответ каждые 128 сэмплов сообщаем, сколько сэмплов лежит в буфере
// (по этому числу основной поток решает, пора ли считать следующий кадр).
class NesAudio extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Float32Array(16384);
    this.r = 0;
    this.w = 0;
    this.fill = 0;
    this.port.onmessage = (e) => {
      const data = e.data;
      const size = this.buf.length;
      for (let i = 0; i < data.length; i++) {
        if (this.fill === size) break; // переполнение: лишнее выбрасываем
        this.buf[this.w] = data[i];
        this.w = (this.w + 1) % size;
        this.fill++;
      }
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0][0];
    const size = this.buf.length;
    for (let i = 0; i < out.length; i++) {
      if (this.fill > 0) {
        out[i] = this.buf[this.r];
        this.r = (this.r + 1) % size;
        this.fill--;
      } else {
        out[i] = 0; // underrun
      }
    }
    this.port.postMessage(this.fill);
    return true;
  }
}

registerProcessor('nes-audio', NesAudio);
