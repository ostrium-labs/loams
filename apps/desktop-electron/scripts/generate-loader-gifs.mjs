// Adapted from dataelement/dsh-desktop (MIT), scripts/generate-loader-gifs.mjs.
// Regenerate the splash loaders (build/loams-loader.gif, build/loams-loader-dark.gif) from
// Loams' mark (web/packages/ui/src/logo.tsx: an L of strata with an indigo seed). Upstream
// rasterises an SVG with sharp; Loams' mark is rectangles, so it is drawn on the cell grid
// directly and encoded with gifenc (MIT): no native image library (W14). The mark bobs
// gently and sheds a few square "bubbles" from its seed, as upstream's loader does.
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import gifenc from 'gifenc'

// gifenc's package entry is CommonJS, so its exports come off the default import.
const { GIFEncoder } = gifenc

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const buildDirectory = path.join(projectRoot, 'build')

const WIDTH = 320
// Square, with headroom above the mark for the bubbles to rise into.
const HEIGHT = 320
const FRAMES = 48
const DELAY_MS = 50
const CELL = 4
const COLS = WIDTH / CELL
const ROWS = HEIGHT / CELL
/** Cells per unit of the mark's 26-unit view box (viewBox="3 3 26 26"). */
const SCALE = 2

/** logo.tsx's full cut: [x, y, width, height] in view-box units; the last is the seed. */
const STRATA = [
  [3, 3, 6, 26],
  [3, 23, 26, 6],
  [12, 16, 14, 4],
  [12, 10, 9, 3],
]
const SEED = [12, 3, 4, 4]

/** Palette index 0 is transparent; 1 the body; 2 the seed; 3–5 the bubbles, fading. */
const THEMES = {
  light: { file: 'loams-loader.gif', palette: [[0, 0, 0], [29, 27, 24], [27, 78, 216], [91, 128, 230], [150, 176, 240], [205, 216, 248]] },
  dark: { file: 'loams-loader-dark.gif', palette: [[0, 0, 0], [245, 245, 245], [119, 162, 252], [86, 120, 214], [56, 82, 160], [34, 48, 96]] },
}

function rng(seed) {
  let state = seed >>> 0
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0
    return state / 0x100000000
  }
}

function markCells() {
  const cells = []
  const add = ([x, y, w, h], colour) => {
    for (let cy = (y - 3) * SCALE; cy < (y - 3 + h) * SCALE; cy += 1) {
      for (let cx = (x - 3) * SCALE; cx < (x - 3 + w) * SCALE; cx += 1) cells.push([cx, cy, colour])
    }
  }
  for (const r of STRATA) add(r, 1)
  add(SEED, 2)
  return cells
}

function buildSparks(origin, random) {
  const sparks = []
  for (let index = 0; index < 8; index += 1) {
    sparks.push({
      start: Math.floor(random() * FRAMES),
      life: 14 + Math.floor(random() * 8),
      x: origin.x + Math.floor(random() * 6) - 1,
      y: origin.y,
      drift: random() < 0.5 ? -1 : 1,
      size: random() < 0.35 ? 1 : 2,
    })
  }
  return sparks
}

function renderFrame(index, mark, sparks, body) {
  const pixels = new Uint8Array(WIDTH * HEIGHT)
  const fillCell = (cx, cy, size, colour) => {
    for (let dy = 0; dy < size; dy += 1) {
      for (let dx = 0; dx < size; dx += 1) {
        const x = cx + dx
        const y = cy + dy
        if (x < 0 || y < 0 || x >= COLS || y >= ROWS) continue
        for (let py = 0; py < CELL; py += 1) pixels.fill(colour, (y * CELL + py) * WIDTH + x * CELL, (y * CELL + py) * WIDTH + x * CELL + CELL)
      }
    }
  }
  const phase = (index / FRAMES) * Math.PI * 2
  const bobY = Math.round(Math.sin(phase) * 2)
  const bobX = Math.round(Math.cos(phase))
  for (const [cx, cy, colour] of mark) fillCell(body.left + cx + bobX, body.top + cy + bobY, 1, colour)
  for (const spark of sparks) {
    const age = (index - spark.start + FRAMES) % FRAMES
    if (age >= spark.life) continue
    const progress = age / spark.life
    const rise = Math.floor(age / 2)
    const sway = Math.round(Math.sin(progress * Math.PI * 2) * spark.drift)
    const shade = 3 + Math.min(2, Math.floor(progress * 3))
    fillCell(spark.x + sway + bobX, spark.y - rise + bobY, progress > 0.75 ? 1 : spark.size, shade)
  }
  return pixels
}

const mark = markCells()
const size = 26 * SCALE
const body = { left: Math.round((COLS - size) / 2), top: Math.round((ROWS - size) / 2) + 2 }
const sparks = buildSparks({ x: body.left + (SEED[0] - 3) * SCALE + 2, y: body.top - 1 }, rng(0x5a17))
for (const theme of Object.values(THEMES)) {
  const gif = GIFEncoder()
  for (let index = 0; index < FRAMES; index += 1) {
    gif.writeFrame(renderFrame(index, mark, sparks, body), WIDTH, HEIGHT, {
      palette: theme.palette,
      delay: DELAY_MS,
      transparent: true,
      transparentIndex: 0,
      repeat: 0,
    })
  }
  gif.finish()
  const destination = path.join(buildDirectory, theme.file)
  await writeFile(destination, gif.bytes())
  console.log(`Wrote ${path.relative(projectRoot, destination)} (${gif.bytes().length} bytes, ${FRAMES} frames)`)
}
