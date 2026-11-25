import { diagram, legend, mindmap, renderInBrowser, svgMeasurer } from 'svg-diagram/browser';

const TONES = ['plain', 'accent', 'new', 'changed', 'removed', 'muted', 'danger', 'solid', 'solid-danger'] as const;
const TAG_TONES = ['accent', 'muted', 'danger'] as const;
const LINES = ['solid', 'dashed', 'dotted'] as const;
type Tone = (typeof TONES)[number];
type TagTone = (typeof TAG_TONES)[number];
type Line = (typeof LINES)[number];
type Tag = { text: string; tone?: TagTone };
type Branch = {
  label: string;
  sub?: string;
  tone?: Tone;
  edge?: { tone?: TagTone; line?: Line };
  tags?: Tag[];
  children?: Branch[];
};
// The legend is fixed text: it never comes from the request.
const LEGEND = [
  { label: '賛成の理由（青の実線）', line: 'solid', arrow: 'none' },
  { label: '反対の理由（赤の破線）', line: 'dashed', arrow: 'none' },
  { label: '決定（濃い青）', tone: 'solid' },
  { label: '宿題（薄い青）', tone: 'accent' },
  { label: '未回答の問い（赤）', tone: 'danger' },
  { label: '要確認（薄い箱。確信度が低い）', tone: 'muted' },
] as const;
type Request = { version: 1; root: Branch };

const oneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === 'string' && (list as readonly string[]).includes(value);

// The input is data from the app; only the library's own names pass. A tag is
// at most 8 code points and non-empty, a branch has at most 3 tags, and the
// root has no edge (svg-diagram rejects it).
function sanitize(branch: Branch, isRoot = true): Branch {
  const { label, sub, tone, edge, tags, children } = branch;
  const cleanTags = (Array.isArray(tags) ? tags : [])
    .filter((tag) => typeof tag?.text === 'string' && tag.text.length > 0 && [...tag.text].length <= 8)
    .slice(0, 3)
    .map((tag) => ({ text: tag.text, ...(oneOf(TAG_TONES, tag.tone) ? { tone: tag.tone } : {}) }));
  const cleanEdge = !isRoot && edge && typeof edge === 'object'
    ? {
        ...(oneOf(TAG_TONES, edge.tone) ? { tone: edge.tone } : {}),
        ...(oneOf(LINES, edge.line) ? { line: edge.line } : {}),
      }
    : undefined;
  return {
    label,
    ...(sub === undefined ? {} : { sub }),
    ...(oneOf(TONES, tone) ? { tone } : {}),
    ...(cleanEdge && Object.keys(cleanEdge).length ? { edge: cleanEdge } : {}),
    ...(cleanTags.length ? { tags: cleanTags } : {}),
    ...(children?.length ? { children: children.map((child) => sanitize(child, false)) } : {}),
  };
}

function depth(branch: Branch): number {
  return 1 + Math.max(0, ...(branch.children ?? []).map(depth));
}

let active = false;

async function render(request: Request) {
  if (request.version !== 1) return { error: 'invalid_input' };
  if (active) return { error: 'busy' };
  active = true;
  try {
    await document.fonts.ready;
    const measure = svgMeasurer();
    try {
      // A right-facing mindmap needs room for each column. The app scrolls the
      // resulting image at its native scale, so the default 940px is a floor.
      const width = Math.max(940, 40 + depth(request.root) * 370);
      const figure = diagram({ title: '議論マップ', appearance: 'light', theme: 'blue', wrap: 'bunsetsu-soft', width, maxWidth: width })
        .add(mindmap(sanitize(request.root), { layout: 'right', maxNodeW: 320 }))
        .add(legend([...LEGEND]));
      const result = await renderInBrowser(figure, { measure });
      if (result.xml.length) return { error: 'invalid_svg' };
      if (!result.verdict.ok) return { error: 'render_failed' };
      if (new TextEncoder().encode(result.svg).length > 1024 * 1024) {
        return { error: 'limit_exceeded' };
      }
      return { svg: result.svg, width: Math.ceil(result.width), warning: result.verdict.warnings.length > 0 };
    } finally {
      measure.dispose();
    }
  } catch {
    return { error: 'render_failed' };
  } finally {
    active = false;
  }
}

if (!globalThis.__meetingramDiagram) {
  Object.defineProperty(globalThis, '__meetingramDiagram', { value: render });
}
