(function installStreams(globalThis) {
if (typeof ReadableStream === 'undefined') {
  globalThis.ReadableStream = class ReadableStream {
    constructor(source = {}, strategy = {}) {
      this._source = source;
      this._queue = [];
      this._reads = [];
      this._closedWaiters = [];
      this._closeRequested = false;
      this._state = "readable";
      this._error = null;
      this.locked = false;
      const stream = this;
      this._controller = {
        enqueue(chunk) {
          if (stream._state !== "readable" || stream._closeRequested) return;
          const pending = stream._reads.shift();
          if (pending) pending.resolve({value: chunk, done: false});
          else stream._queue.push(chunk);
        },
        close() {
          if (stream._state !== "readable") return;
          stream._closeRequested = true;
          if (stream._queue.length) return;
          stream._state = "closed";
          while (stream._reads.length) {
            stream._reads.shift().resolve({value: undefined, done: true});
          }
          while (stream._closedWaiters.length) stream._closedWaiters.shift().resolve();
        },
        error(error) {
          if (stream._state !== "readable") return;
          stream._state = "errored";
          stream._error = error;
          stream._queue.length = 0;
          while (stream._reads.length) stream._reads.shift().reject(error);
          while (stream._closedWaiters.length) stream._closedWaiters.shift().reject(error);
        },
        get desiredSize() { return Math.max(0, 1 - stream._queue.length); },
      };
      try {
        const started = source.start?.(this._controller);
        if (started && typeof started.then === "function") {
          started.catch((error) => this._controller.error(error));
        }
      } catch (error) {
        this._controller.error(error);
      }
    }
    getReader() {
      if (this.locked) throw new TypeError("ReadableStream is locked");
      this.locked = true;
      const stream = this;
      let closedPromise;
      return {
        read() {
          if (stream._queue.length > 0) {
            const value = stream._queue.shift();
            if (!stream._queue.length && stream._closeRequested) stream._controller.close();
            return Promise.resolve({ value, done: false });
          }
          if (stream._state === "closed") return Promise.resolve({ value: undefined, done: true });
          if (stream._state === "errored") return Promise.reject(stream._error);
          return new Promise((resolve, reject) => stream._reads.push({resolve, reject}));
        },
        releaseLock() { stream.locked = false; },
        cancel(reason) { return stream.cancel(reason); },
        get closed() {
          if (!closedPromise) {
            closedPromise = new Promise((resolve, reject) => {
              if (stream._state === "closed") resolve();
              else if (stream._state === "errored") reject(stream._error);
              else stream._closedWaiters.push({resolve, reject});
            });
            closedPromise.catch(() => {});
          }
          return closedPromise;
        },
      };
    }
    cancel(reason) {
      if (this._state === "errored") return Promise.reject(this._error);
      this._queue.length = 0;
      if (this._state === "closed") return Promise.resolve();
      this._controller.close();
      try { return Promise.resolve(this._source.cancel?.(reason)); }
      catch (error) { return Promise.reject(error); }
    }
    async pipeTo(destination) {
      const reader = this.getReader();
      const writer = destination.getWriter();
      try {
        while (true) {
          const {value, done} = await reader.read();
          if (done) break;
          await writer.write(value);
        }
        await writer.close();
      } catch (error) {
        try { await writer.abort(error); } catch {}
        throw error;
      } finally {
        reader.releaseLock();
        writer.releaseLock();
      }
    }
    pipeThrough(transform) {
      this.pipeTo(transform.writable).catch((error) => {
        try { transform.readable._controller?.error(error); } catch {}
      });
      return transform.readable;
    }
    tee() {
      let leftController;
      let rightController;
      const left = new ReadableStream({start(controller) { leftController = controller; }});
      const right = new ReadableStream({start(controller) { rightController = controller; }});
      (async () => {
        try {
          const reader = this.getReader();
          while (true) {
            const {value, done} = await reader.read();
            if (done) break;
            leftController.enqueue(value);
            rightController.enqueue(value);
          }
          leftController.close();
          rightController.close();
        } catch (error) {
          leftController.error(error);
          rightController.error(error);
        }
      })();
      return [left, right];
    }
    [Symbol.asyncIterator]() {
      const reader = this.getReader();
      return { next: () => reader.read(), return: () => { reader.releaseLock(); return Promise.resolve({done:true}); } };
    }
  };
}
if (typeof WritableStream === 'undefined') {
  globalThis.WritableStream = class WritableStream {
    constructor(sink = {}) {
      this._sink = sink;
      this._state = "writable";
      this._error = null;
      this._chain = Promise.resolve();
      this.locked = false;
      try {
        const started = sink.start?.({});
        if (started && typeof started.then === "function") this._chain = Promise.resolve(started);
      } catch (error) {
        this._state = "errored";
        this._error = error;
        this._chain = Promise.reject(error);
      }
    }
    getWriter() {
      if (this.locked) throw new TypeError("WritableStream is locked");
      this.locked = true;
      const stream = this;
      return {
        write(chunk) {
          if (stream._state !== "writable") return Promise.reject(stream._error || new TypeError("WritableStream is closed"));
          stream._chain = stream._chain.then(() => stream._sink.write?.(chunk));
          return stream._chain;
        },
        close() {
          if (stream._state !== "writable") return stream._chain;
          stream._state = "closed";
          stream._chain = stream._chain.then(() => stream._sink.close?.());
          return stream._chain;
        },
        abort(reason) {
          stream._state = "errored";
          stream._error = reason;
          stream._chain = stream._chain.then(() => stream._sink.abort?.(reason));
          return stream._chain;
        },
        releaseLock() { stream.locked = false; },
        get ready() { return stream._chain.then(() => undefined); },
        get closed() { return stream._chain.then(() => undefined); },
        get desiredSize() { return 1; },
      };
    }
    close() { const writer = this.getWriter(); return writer.close().finally(() => writer.releaseLock()); }
    abort(reason) { const writer = this.getWriter(); return writer.abort(reason).finally(() => writer.releaseLock()); }
  };
}
if (typeof TransformStream === 'undefined') {
  globalThis.TransformStream = class TransformStream {
    constructor(transformer = {}) {
      let controller;
      this.readable = new ReadableStream({
        start(readableController) { controller = readableController; },
      });
      this.writable = new WritableStream({
        async write(chunk) {
          try {
            if (transformer.transform) await transformer.transform(chunk, controller);
            else controller.enqueue(chunk);
          } catch (error) { controller.error(error); throw error; }
        },
        async close() {
          try {
            if (transformer.flush) await transformer.flush(controller);
            controller.close();
          } catch (error) { controller.error(error); throw error; }
        },
        abort(reason) { controller.error(reason); },
      });
      try { transformer.start?.(controller); }
      catch (error) { controller.error(error); }
    }
  };
}
if (typeof TextEncoderStream === 'undefined') {
  globalThis.TextEncoderStream = class TextEncoderStream {
    constructor() {
      const encoder = new TextEncoder();
      const transform = new TransformStream({
        transform(chunk, controller) {
          controller.enqueue(encoder.encode(String(chunk)));
        },
      });
      this.readable = transform.readable;
      this.writable = transform.writable;
    }
    get encoding() { return "utf-8"; }
  };
}
if (typeof TextDecoderStream === 'undefined') {
  globalThis.TextDecoderStream = class TextDecoderStream {
    constructor(label = "utf-8", options = {}) {
      const decoder = new TextDecoder(label, options);
      const transform = new TransformStream({
        transform(chunk, controller) {
          const text = decoder.decode(chunk, {stream: true});
          if (text) controller.enqueue(text);
        },
        flush(controller) {
          const tail = decoder.decode();
          if (tail) controller.enqueue(tail);
        },
      });
      this.readable = transform.readable;
      this.writable = transform.writable;
      this._decoder = decoder;
    }
    get encoding() { return this._decoder.encoding; }
    get fatal() { return this._decoder.fatal; }
    get ignoreBOM() { return this._decoder.ignoreBOM; }
  };
}
})
