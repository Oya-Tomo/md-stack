// Bundled into assets/mathjax.js and executed by md-stack inside an embedded JS engine.
// Exposes `mdStackRenderTex(texSources, displayFlags)` which renders a batch of TeX
// expressions to self-contained SVG, sharing macro definitions within the batch only.
import { mathjax } from 'mathjax-full/js/mathjax.js';
import { TeX } from 'mathjax-full/js/input/tex.js';
import { SVG } from 'mathjax-full/js/output/svg.js';
import { liteAdaptor } from 'mathjax-full/js/adaptors/liteAdaptor.js';
import { RegisterHTMLHandler } from 'mathjax-full/js/handlers/html.js';
import { AllPackages } from 'mathjax-full/js/input/tex/AllPackages.js';

const adaptor = liteAdaptor();
RegisterHTMLHandler(adaptor);

// Packages that would hide errors or need asynchronous loading are excluded so that
// every failure surfaces as an exception.
const EXCLUDED = ['noerrors', 'noundefined', 'autoload', 'require'];
const packages = AllPackages.filter((p) => !EXCLUDED.includes(p));

const exValue = (attr) => parseFloat(attr ?? '0');

function newDocument() {
  const tex = new TeX({
    packages,
    formatError: (_jax, err) => {
      throw err;
    },
  });
  const svg = new SVG({ fontCache: 'none' });
  return mathjax.document('', { InputJax: tex, OutputJax: svg });
}

globalThis.mdStackRenderTex = (sources, displays) => {
  const doc = newDocument();
  return JSON.stringify(
    sources.map((source, i) => {
      try {
        const container = doc.convert(source, { display: displays[i] });
        const svg = adaptor.firstChild(container);
        const style = adaptor.getAttribute(svg, 'style') ?? '';
        const valign = /vertical-align:\s*(-?[\d.]+)ex/.exec(style);
        return {
          ok: {
            svg: adaptor.outerHTML(svg),
            width_ex: exValue(adaptor.getAttribute(svg, 'width')),
            height_ex: exValue(adaptor.getAttribute(svg, 'height')),
            depth_ex: valign ? -parseFloat(valign[1]) : 0,
          },
        };
      } catch (e) {
        return { err: String(e?.message ?? e) };
      }
    }),
  );
};
