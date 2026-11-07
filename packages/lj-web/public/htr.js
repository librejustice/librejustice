// The slow reader, which also reads handwriting: one module per worker, the
// lines of a page shared out between them. The site sends no COOP/COEP, so the
// workers share no memory, and each holds its own copy of the networks.
window.ljHtrReady = (async () => {
  const base = new URL("/redactor/htr", import.meta.url).href;
  // A worker starts from the page's own origin, so a view framed elsewhere starts it
  // from a blob that imports it.
  const script = new URL("/htr-worker.js", import.meta.url).href;
  const worker = new URL(import.meta.url).origin === location.origin
    ? script
    : URL.createObjectURL(new Blob([`import ${JSON.stringify(script)};`], { type: "text/javascript" }));
  // French first: the handwriting reader reads it alone, the others tell a page it cannot read.
  const languages = ["fr", "en", "de", "es", "it", "pt", "nl"];
  const files = ["det.onnx", "rec.onnx", "dict.txt", "laia.onnx", "syms.txt", ...languages.map((l) => `lexicon-${l}.txt`)];
  // Up to four workers, which read a page several times faster than one.
  const size = Math.max(1, Math.min(4, (navigator.hardwareConcurrency || 2) - 1));
  let starting = null;

  class Slot {
    constructor(module, networks) {
      this.worker = new Worker(worker, { type: "module" });
      this.pending = new Map();
      this.next = 0;
      this.worker.onmessage = ({ data }) => {
        const { resolve, reject } = this.pending.get(data.id);
        this.pending.delete(data.id);
        if (data.error) reject(new Error(data.error));
        else resolve(data.result);
      };
      this.ready = this.ask("start", { module, networks });
    }
    ask(kind, body) {
      const id = this.next++;
      return new Promise((resolve, reject) => {
        this.pending.set(id, { resolve, reject });
        this.worker.postMessage({ id, kind, ...body });
      });
    }
  }

  async function start(report) {
    let done = 0;
    const step = () => report(++done / (files.length + 1));
    const [module, ...networks] = await Promise.all([
      WebAssembly.compileStreaming(fetch(`${base}/lj_htr_bg.wasm`)).finally(step),
      ...files.map((name) =>
        fetch(`${base}/models/${name}`).then((r) => {
          if (!r.ok) throw new Error(`${name} : ${r.status}`);
          return name.endsWith(".txt") ? r.text() : r.arrayBuffer();
        }).finally(step)),
    ]);
    const workers = Array.from({ length: size }, () => new Slot(module, networks));
    await Promise.all(workers.map((w) => w.ready));
    return workers;
  }

  const reader = {
    progress: null,
    prepare() {
      starting ??= start((at) => reader.progress?.("loading models", at)).catch((error) => {
        starting = null;
        throw error;
      });
      return starting;
    },
    async words(image, page) {
      const pool = await reader.prepare();
      const { data, width, height } = image.getContext("2d").getImageData(0, 0, image.width, image.height);
      const grey = new Uint8Array(width * height);
      for (let i = 0; i < grey.length; i++) grey[i] = data[4 * i];
      reader.progress?.("recognizing text", 0);
      await Promise.all(pool.map((w) => w.ask("sheet", { grey, width })));
      // Each worker detects a band of the page, and the bands are cut into lines as one map.
      const bands = await Promise.all(pool.map((w, index) => w.ask("band", { index, count: pool.length })));
      const map = new Float32Array(bands.reduce((n, b) => n + b.length, 0));
      bands.reduce((at, b) => (map.set(b, at), at + b.length), 0);
      const segments = await pool[0].ask("segments", { map });
      // Line by line, then by batches of doubtful lines, whichever worker is free.
      let done = 0;
      let total = segments.length;
      const share = async (jobs, run) => {
        let next = 0;
        await Promise.all(pool.map(async (worker) => {
          for (let at = next++; at < jobs.length; at = next++) {
            await run(worker, jobs[at]);
            done += jobs[at].length;
            reader.progress?.("recognizing text", done / total);
          }
        }));
      };
      const lines = new Array(segments.length);
      await share(segments.map((s, i) => [i]), async (worker, [i]) => {
        [lines[i]] = await worker.ask("printed", { segments: [segments[i]] });
      });
      const batches = await pool[0].ask("batches", { segments, lines });
      total += batches.reduce((n, b) => n + b.length, 0);
      await share(batches, async (worker, batch) => {
        const read = await worker.ask("hand", { segments: batch.map((i) => segments[i]) });
        batch.forEach((i, k) => { lines[i].hand = read[k]; });
      });
      const found = await pool[0].ask("page", { lines });
      found.sort((a, b) => a.line - b.line || a.x - b.x);
      return found.map((w) => ({ page, ...w }));
    },
  };
  return reader;
})();
