// Tesseract is Emscripten-built, so it is loaded the way it expects. The worker
// is kept between pages: creating one re-fetches the core and its data.
window.ljOcrReady = (async () => {
  // The ESM build exports the whole library as its default and names nothing.
  const { createWorker } = (await import("/ocr/tesseract.esm.min.js")).default;
  const files = new URL("/ocr", import.meta.url).href;
  const core = `${files}/tesseract-core-simd-lstm.js`;
  // A worker starts from the page's own origin, so a view framed elsewhere starts it
  // from a blob, where Emscripten looks for its wasm beside `__filename`.
  const workerPath = new URL(import.meta.url).origin === location.origin
    ? `${files}/worker.min.js`
    : URL.createObjectURL(new Blob(
      [`self.__filename = ${JSON.stringify(core)}; importScripts(${JSON.stringify(`${files}/worker.min.js`)});`],
      { type: "text/javascript" },
    ));
  let worker = null;
  // The worker holds its logger for its whole life, so the callback it calls is
  // this stable field and never the one handed to a single call.
  const reader = {
    progress: null,
    prepare() {
      worker ??= createWorker("fra", 1, {
        // Chosen above: tesseract.js's own blob would hide the wasm's folder.
        workerBlobURL: false,
        workerPath,
        corePath: core,
        langPath: files,
        gzip: false,
        logger: (m) => reader.progress?.(m.status ?? "", m.progress ?? 0),
      }).catch((error) => {
        worker = null;
        throw error;
      });
      return worker;
    },
    async words(image, page) {
      const { data } = await (await reader.prepare()).recognize(image, {}, { blocks: true });
      const words = [];
      let line = 0;
      for (const block of data.blocks ?? []) {
        for (const paragraph of block.paragraphs ?? []) {
          for (const row of paragraph.lines ?? []) {
            for (const word of row.words ?? []) {
              const b = word.bbox;
              words.push({
                page,
                line,
                text: word.text,
                x: b.x0,
                y: b.y0,
                width: b.x1 - b.x0,
                height: b.y1 - b.y0,
              });
            }
            line += 1;
          }
        }
      }
      return words;
    },
  };
  return reader;
})();
