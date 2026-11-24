import { diagram, mindmap, renderInBrowser, svgMeasurer } from 'svg-diagram/browser';

type Branch = { label: string; sub?: string; children?: Branch[] };
type Request = { version: 1; root: Branch };

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
      const figure = diagram({ title: '議論マップ', appearance: 'light', theme: 'blue', width, maxWidth: width })
        .add(mindmap(request.root, { layout: 'right', maxNodeW: 320 }));
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
