import http from 'node:http';
import { AutoConfig, AutoModel, AutoProcessor, env, load_image } from '@huggingface/transformers';

const MODEL_ID = 'onnx-community/embeddinggemma-2-ONNX';
const MODEL_REVISION = 'daa72c51243991dfcaf9f9137d2c573d8f7790c0';
const OUTPUT_DIMENSIONS = 512;
const NATIVE_DIMENSIONS = 768;
const MAX_BODY_BYTES = 14 * 1024 * 1024;
const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
const MAX_BATCH_SIZE = 16;
const MAX_TEXT_CHARS = 16_000;
const MAX_PENDING_REQUESTS = 3;
const PORT = Number(process.env.PORT ?? 8788);

env.cacheDir = process.env.MODEL_CACHE ?? '/data/hf';
if (env.backends?.onnx?.wasm) {
  env.backends.onnx.wasm.numThreads = Number(process.env.ORT_THREADS ?? 2);
}

let model;
let processor;
let pendingRequests = 0;
let inferenceQueue = Promise.resolve();

function json(response, status, body) {
  response.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
  });
  response.end(JSON.stringify(body));
}

async function readJson(request) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of request) {
    bytes += chunk.length;
    if (bytes > MAX_BODY_BYTES) {
      const error = new Error('body_too_large');
      error.status = 413;
      throw error;
    }
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    const error = new Error('invalid_json');
    error.status = 400;
    throw error;
  }
}

function reduceAndNormalize(output) {
  const tensor = output?.sentence_embedding;
  if (!tensor || tensor.dims.at(-1) !== NATIVE_DIMENSIONS) {
    throw new Error('unexpected_embedding_shape');
  }
  const batchSize = tensor.dims.length === 1 ? 1 : tensor.dims[0];
  const vectors = [];
  for (let row = 0; row < batchSize; row += 1) {
    const start = row * NATIVE_DIMENSIONS;
    const vector = Array.from(tensor.data.subarray(start, start + OUTPUT_DIMENSIONS));
    let normSquared = 0;
    for (const value of vector) {
      if (!Number.isFinite(value)) throw new Error('non_finite_embedding');
      normSquared += value * value;
    }
    const norm = Math.sqrt(normSquared);
    if (!Number.isFinite(norm) || norm <= 1e-12) throw new Error('zero_embedding');
    vectors.push(vector.map((value) => value / norm));
  }
  return vectors;
}

async function embedTexts(inputs) {
  const texts = typeof inputs === 'string' ? [inputs] : inputs;
  if (!Array.isArray(texts) || texts.length === 0 || texts.length > MAX_BATCH_SIZE) {
    const error = new Error('invalid_batch_size');
    error.status = 400;
    throw error;
  }
  if (texts.some((text) => typeof text !== 'string' || text.length > MAX_TEXT_CHARS)) {
    const error = new Error('invalid_text');
    error.status = 400;
    throw error;
  }
  const inputsForModel = await processor(texts, null, null, null, {
    padding: true,
    truncation: true,
    max_length: 256,
  });
  const output = await model(inputsForModel);
  return reduceAndNormalize(output);
}

async function embedImage(imageBase64) {
  if (typeof imageBase64 !== 'string' || imageBase64.length === 0 || imageBase64.length > MAX_IMAGE_BYTES * 1.34) {
    const error = new Error('invalid_image');
    error.status = 400;
    throw error;
  }
  const imageBytes = Buffer.from(imageBase64, 'base64');
  if (imageBytes.length === 0 || imageBytes.length > MAX_IMAGE_BYTES) {
    const error = new Error('invalid_image');
    error.status = 400;
    throw error;
  }
  const image = await load_image(new Blob([imageBytes]));
  const inputsForModel = await processor(null, image);
  const output = await model(inputsForModel);
  return reduceAndNormalize(output)[0];
}

async function runInference(work) {
  if (pendingRequests >= MAX_PENDING_REQUESTS) {
    const error = new Error('encoder_busy');
    error.status = 503;
    throw error;
  }
  pendingRequests += 1;
  const result = inferenceQueue.then(work, work);
  inferenceQueue = result.then(() => undefined, () => undefined);
  try {
    return await result;
  } finally {
    pendingRequests -= 1;
  }
}

async function handle(request, response) {
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');
  if (request.method === 'GET' && url.pathname === '/healthz') {
    return json(response, model ? 200 : 503, {
      ready: Boolean(model && processor),
      model: MODEL_ID,
      revision: MODEL_REVISION,
      dimensions: OUTPUT_DIMENSIONS,
      quantization: 'q4',
    });
  }
  if (request.method !== 'POST' || !['/embed', '/embed-image'].includes(url.pathname)) {
    return json(response, 404, { error: 'not_found' });
  }
  try {
    const body = await readJson(request);
    if (url.pathname === '/embed') {
      const vectors = await runInference(() => embedTexts(body.inputs));
      return json(response, 200, vectors);
    }
    const vector = await runInference(() => embedImage(body.image_base64));
    return json(response, 200, vector);
  } catch (error) {
    const status = Number.isInteger(error?.status) ? error.status : 500;
    if (status >= 500) console.error('embedding_request_failed', error?.message ?? 'unknown_error');
    return json(response, status, { error: status >= 500 ? 'embedding_failed' : error.message });
  }
}

async function main() {
  const config = await AutoConfig.from_pretrained(MODEL_ID, { revision: MODEL_REVISION });
  config.audio_config = null;
  processor = await AutoProcessor.from_pretrained(MODEL_ID, { revision: MODEL_REVISION });
  model = await AutoModel.from_pretrained(MODEL_ID, {
    revision: MODEL_REVISION,
    config,
    device: 'cpu',
    dtype: 'q4',
  });

  const server = http.createServer((request, response) => {
    void handle(request, response);
  });
  server.listen(PORT, '0.0.0.0', () => {
    console.log(`embedding_service_ready port=${PORT} model=${MODEL_ID}@${MODEL_REVISION} dim=${OUTPUT_DIMENSIONS}`);
  });
  const shutdown = async () => {
    server.close();
    await model?.dispose();
    process.exit(0);
  };
  process.once('SIGINT', shutdown);
  process.once('SIGTERM', shutdown);
}

main().catch((error) => {
  console.error('embedding_service_start_failed', error?.message ?? 'unknown_error');
  process.exitCode = 1;
});
