import { $f as f32, Ac as defineStd, G as orientedBoxField, Hd as call, Jd as vec4, Ld as sampleCatmullRomExpr, _d as createFontDependentRaster, gd as createCanvasRasterTarget, hd as loadGoogleFont, md as isFontAvailable, vd as createRasterInvalidator, yd as createSwappableMediaTexture } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
function applyTextTransform(text, mode) {
	switch (mode) {
		case "uppercase": return text.toUpperCase();
		case "lowercase": return text.toLowerCase();
		case "capitalize": return text.replace(/(^|\s)(\S)/g, (_, ws, ch) => ws + ch.toUpperCase());
		default: return text;
	}
}
var generation = 0;
function bumpMeasureGeneration() {
	generation++;
}
var measureCtx = null;
var letterSpacingSupported = false;
function getMeasureContext() {
	if (measureCtx) return measureCtx;
	if (typeof document === "undefined") return null;
	const canvas = document.createElement("canvas");
	canvas.width = 1;
	canvas.height = 1;
	measureCtx = canvas.getContext("2d", { willReadFrequently: false });
	if (measureCtx) letterSpacingSupported = "letterSpacing" in measureCtx;
	return measureCtx;
}
function supportsCanvasLetterSpacing() {
	getMeasureContext();
	return letterSpacingSupported;
}
var MAX_CACHE_ENTRIES = 500;
var cache = /* @__PURE__ */ new Map();
function measureText(spec) {
	const text = applyTextTransform(spec.text, spec.textTransform);
	const ctx = getMeasureContext();
	if (!ctx) return {
		widthPer100: text.length * 55,
		ascentPer100: 80,
		descentPer100: 20,
		inkAscentPer100: 80,
		inkDescentPer100: 20
	};
	const fontLoaded = isFontAvailable(spec.fontFamily, spec.fontWeight, spec.italic);
	const key = `${generation}|${fontLoaded ? 1 : 0}|${spec.fontFamily}|${spec.fontWeight}|${spec.italic ? 1 : 0}|${spec.letterSpacingEm}|${text}`;
	const cached = cache.get(key);
	if (cached) return cached;
	ctx.font = `${spec.italic ? "italic " : ""}${spec.fontWeight} 100px "${spec.fontFamily}", sans-serif`;
	if (letterSpacingSupported) ctx.letterSpacing = `${spec.letterSpacingEm * 100}px`;
	const metrics = ctx.measureText(text);
	let widthPer100 = metrics.width;
	if (letterSpacingSupported && text.length > 0) widthPer100 = Math.max(0, widthPer100 - spec.letterSpacingEm * 100);
	const ascentPer100 = metrics.fontBoundingBoxAscent ?? metrics.actualBoundingBoxAscent ?? 80;
	const descentPer100 = metrics.fontBoundingBoxDescent ?? metrics.actualBoundingBoxDescent ?? 20;
	const rawInkAsc = metrics.actualBoundingBoxAscent;
	const rawInkDesc = metrics.actualBoundingBoxDescent;
	const hasInk = rawInkAsc !== void 0 && rawInkDesc !== void 0 && rawInkAsc + rawInkDesc > 1;
	const result = {
		widthPer100,
		ascentPer100,
		descentPer100,
		inkAscentPer100: hasInk ? rawInkAsc : ascentPer100,
		inkDescentPer100: hasInk ? rawInkDesc : descentPer100
	};
	if (cache.size >= MAX_CACHE_ENTRIES) cache.clear();
	cache.set(key, result);
	return result;
}
function wrapLines(text, maxWidthPer100, measureLine) {
	const explicit = text.split(/\r\n|\r|\n/);
	if (!(maxWidthPer100 > 0)) return explicit;
	const out = [];
	for (const line of explicit) {
		const words = line.split(" ");
		let current = "";
		for (const word of words) {
			const candidate = current ? `${current} ${word}` : word;
			if (current && measureLine(candidate) > maxWidthPer100) {
				out.push(current);
				current = word;
			} else current = candidate;
		}
		out.push(current);
	}
	return out;
}
function measureTextBlock(spec, lineHeightEm, maxWidthPer100) {
	const lines = wrapLines(applyTextTransform(spec.text, spec.textTransform), maxWidthPer100 ?? 0, (l) => measureText({
		...spec,
		text: l,
		textTransform: "none"
	}).widthPer100);
	const per = lines.map((l) => measureText({
		...spec,
		text: l,
		textTransform: "none"
	}));
	const n = Math.max(1, lines.length);
	const first = per[0] ?? measureText(spec);
	const last = per[per.length - 1] ?? first;
	const pitchPer100 = Math.max(1, lineHeightEm * 100);
	return {
		lines,
		lineWidthsPer100: per.map((m) => m.widthPer100),
		widthPer100: per.reduce((w, m) => Math.max(w, m.widthPer100), 0),
		ascentPer100: first.ascentPer100,
		descentPer100: first.descentPer100,
		inkAscentFirstPer100: first.inkAscentPer100,
		inkDescentLastPer100: last.inkDescentPer100,
		pitchPer100,
		blockHeightPer100: first.ascentPer100 + (n - 1) * pitchPer100 + first.descentPer100,
		inkHeightPer100: first.inkAscentPer100 + (n - 1) * pitchPer100 + last.inkDescentPer100
	};
}
var MAX_TEXTURE_AXIS = 4096;
var TEXT_SUPERSAMPLE = 3;
var wrapWidthPxOf = (v, canvasWidth) => {
	if (v && typeof v === "object" && typeof v.value === "number") return v.unit === "px" ? v.value : v.value * canvasWidth;
	return (typeof v === "number" ? v : 0) * canvasWidth;
};
var measureBlockOf = (props, cw, ch) => {
	const fontSizePx = fontSizePxOf(props.fontSize, ch);
	const widthPx = wrapWidthPxOf(props.width, cw);
	return {
		fontSizePx,
		widthPx,
		m: measureTextBlock({
			text: String(props.text ?? ""),
			fontFamily: String(props.fontFamily ?? "Inter"),
			fontWeight: Number(props.fontWeight ?? 400),
			italic: !!props.italic,
			letterSpacingEm: Number(props.letterSpacing ?? 0),
			textTransform: props.textTransform ?? "none"
		}, Number(props.lineHeight ?? 1.2) || 1.2, fontSizePx > 0 && widthPx > 0 ? widthPx * 100 / fontSizePx : 0)
	};
};
var fontSizePxOf = (v, canvasHeight) => {
	if (v && typeof v === "object" && typeof v.value === "number") return v.unit === "px" ? v.value : v.value * canvasHeight;
	return (typeof v === "number" ? v : .1) * canvasHeight;
};
function createGlyphRasterHost(params) {
	const { dimensions, getCpuValue, setExtraField } = params;
	const target = createCanvasRasterTarget(params, {
		maxAxis: MAX_TEXTURE_AXIS,
		supersample: TEXT_SUPERSAMPLE
	});
	const rasterCtx = target.context();
	const glyphTex = createSwappableMediaTexture(params, {
		label: "Text:glyph",
		initial: {
			width: 2,
			height: 2
		}
	});
	const defaults = componentDefinition.props;
	const readProps = () => ({
		text: String(getCpuValue("text") ?? defaults.text.default),
		fontFamily: String(getCpuValue("fontFamily") ?? defaults.fontFamily.default),
		fontWeight: Number(getCpuValue("fontWeight") ?? defaults.fontWeight.default),
		italic: Number(getCpuValue("italic") ?? 0) > .5,
		fontSizeUV: Number(getCpuValue("fontSize") ?? defaults.fontSize.default),
		letterSpacing: Number(getCpuValue("letterSpacing") ?? defaults.letterSpacing.default),
		lineHeight: Number(getCpuValue("lineHeight") ?? defaults.lineHeight.default) || defaults.lineHeight.default,
		textAlign: String(getCpuValue("textAlign") ?? defaults.textAlign.default),
		widthUV: Number(getCpuValue("width") ?? defaults.width.default),
		textTransform: String(getCpuValue("textTransform") ?? defaults.textTransform.default)
	});
	const raster = () => {
		if (!rasterCtx || target.disposed) return;
		const p = readProps();
		const dimW = dimensions.width || 1;
		const dimH = dimensions.height || 1;
		const fontSizePx = p.fontSizeUV * dimH;
		const wrapWidthPx = p.widthUV * dimW;
		if (!p.text.trim() || fontSizePx <= 0) {
			target.resize(2, 2);
			glyphTex.ensureSize(2, 2);
			glyphTex.write(target.canvas());
			setExtraField("halfW", .001);
			setExtraField("halfH", .001);
			return;
		}
		const m = measureTextBlock({
			text: p.text,
			fontFamily: p.fontFamily,
			fontWeight: p.fontWeight,
			italic: p.italic,
			letterSpacingEm: p.letterSpacing,
			textTransform: p.textTransform
		}, p.lineHeight, wrapWidthPx > 0 ? wrapWidthPx * 100 / fontSizePx : 0);
		const f = fontSizePx / 100;
		const contentWPx = Math.max(1, (wrapWidthPx > 0 ? Math.max(wrapWidthPx * 100 / fontSizePx, m.widthPer100) : m.widthPer100) * f);
		const pitchPx = m.pitchPer100 * f;
		const lineBoxPx = Math.max(1, fontSizePx * p.lineHeight);
		const blockHPx = Math.max(lineBoxPx, m.blockHeightPer100 * f);
		const ascentPx = m.ascentPer100 * f;
		const descentPx = m.descentPer100 * f;
		const padPx = Math.max(.25 * fontSizePx, (ascentPx + descentPx - lineBoxPx) / 2 + .25 * fontSizePx);
		const cssW = contentWPx + padPx * 2;
		const cssH = blockHPx + padPx * 2;
		const { scale, texW, texH } = target.rasterSize(cssW, cssH);
		target.resize(texW, texH);
		rasterCtx.fillStyle = "#ffffff";
		rasterCtx.font = `${p.italic ? "italic " : ""}${p.fontWeight} ${fontSizePx * scale}px "${p.fontFamily}", sans-serif`;
		if (supportsCanvasLetterSpacing()) rasterCtx.letterSpacing = `${p.letterSpacing * fontSizePx * scale}px`;
		rasterCtx.textAlign = "left";
		rasterCtx.textBaseline = "alphabetic";
		const blockTopY = texH / 2 - blockHPx / 2 * scale;
		const contentLeftX = (texW - contentWPx * scale) / 2;
		for (let i = 0; i < m.lines.length; i++) {
			const line = m.lines[i];
			if (!line.trim()) continue;
			const lineWPx = m.lineWidthsPer100[i] * f;
			const drawX = p.textAlign === "left" ? contentLeftX : p.textAlign === "right" ? contentLeftX + (contentWPx - lineWPx) * scale : contentLeftX + (contentWPx - lineWPx) / 2 * scale;
			const baselineY = blockTopY + (ascentPx + i * pitchPx) * scale;
			rasterCtx.fillText(line, drawX, baselineY);
		}
		glyphTex.ensureSize(texW, texH);
		glyphTex.write(target.canvas());
		setExtraField("halfW", Math.max(1e-4, cssW / 2 / dimH));
		setExtraField("halfH", Math.max(1e-4, cssH / 2 / dimH));
	};
	const font = createFontDependentRaster({
		key: () => {
			const p = readProps();
			return `${p.fontFamily}|${p.fontWeight}|${p.italic}`;
		},
		load: () => {
			const p = readProps();
			return loadGoogleFont(p.fontFamily, p.fontWeight, p.italic);
		},
		onLoaded: () => {
			bumpMeasureGeneration();
			raster();
		}
	});
	setTimeout(() => {
		raster();
		font.ensure();
	}, 0);
	createRasterInvalidator(params, {
		key: () => {
			const p = readProps();
			return `${p.text}|${p.fontFamily}|${p.fontWeight}|${p.italic}|${p.fontSizeUV}|${p.letterSpacing}|${p.lineHeight}|${p.textAlign}|${p.widthUV}|${p.textTransform}|${dimensions.width}x${dimensions.height}|${params.canvas.width}`;
		},
		run: () => {
			raster();
			font.ensure();
		}
	});
	return glyphTex;
}
var glyphFrame = ({ uniforms, ctx, uvContext }) => call(orientedBoxField, "orientedBoxField", [
	uvContext ?? ctx.uv,
	ctx.aspect,
	uniforms.center,
	uniforms.rotation,
	uniforms.halfW,
	uniforms.halfH
]);
var glyphTap = (glyph, frame) => sampleCatmullRomExpr(glyph, frame.member("xy"), "linearClamp", "unit");
var tintGlyph = (sampled, frame, color) => vec4(color.member("rgb"), sampled.member("a").mul(color.member("a")).mul(frame.member("z")));
const componentDefinition = defineStd({
	name: "Text",
	role: "media",
	species: "custom",
	category: "Textures",
	description: "Text with any Google font — multi-line with wrapping, alignment and line height — rendered crisp via a glyph texture",
	acceptsUVContext: true,
	extraFields: {
		halfW: {
			schema: f32,
			initial: .001
		},
		halfH: {
			schema: f32,
			initial: .001
		}
	},
	boundingBoxDeclaration: {
		propBindings: {
			x: {
				prop: "center",
				as: "position-x"
			},
			y: {
				prop: "center",
				as: "position-y"
			},
			rotation: {
				prop: "rotation",
				as: "degrees"
			}
		},
		computeBounds(props, cw, ch) {
			const { fontSizePx, widthPx, m } = measureBlockOf(props, cw, ch);
			const f = fontSizePx / 100;
			const cx = typeof props.center?.x === "number" ? props.center.x : .5;
			const cy = typeof props.center?.y === "number" ? props.center.y : .5;
			const inkShiftPx = (m.ascentPer100 - m.descentPer100 + (m.inkDescentLastPer100 - m.inkAscentFirstPer100)) / 2 * f;
			return {
				centerXPx: cx * cw,
				centerYPx: cy * ch + inkShiftPx,
				widthPx: Math.max(0, widthPx > 0 ? widthPx : m.widthPer100 * f),
				heightPx: Math.max(0, m.inkHeightPer100 * f),
				rotationDeg: props.rotation ?? 0
			};
		},
		writeBounds(bounds, currentProps, cw, ch) {
			const { fontSizePx: oldFontPx, widthPx: oldWidthPx, m } = measureBlockOf(currentProps, cw, ch);
			const oldInkPx = Math.max(1, m.inkHeightPer100 * oldFontPx / 100);
			let scale = bounds.heightPx / oldInkPx;
			if (oldWidthPx > 0) {
				const sW = bounds.widthPx / Math.max(1, oldWidthPx);
				const sH = bounds.heightPx / oldInkPx;
				scale = Math.abs(sW - sH) < .02 ? sH : 1;
			}
			const newFontPx = Math.max(1, oldFontPx * scale);
			const inkShiftPx = (m.ascentPer100 - m.descentPer100 + (m.inkDescentLastPer100 - m.inkAscentFirstPer100)) / 2 * (newFontPx / 100);
			const rawFont = currentProps.fontSize;
			const fontSize = rawFont && typeof rawFont === "object" && rawFont.unit === "px" ? {
				value: Math.max(1, Math.round(newFontPx)),
				unit: "px"
			} : Math.max(.001, ch > 0 ? newFontPx / ch : .001);
			const updates = {
				center: {
					...currentProps.center ?? {
						x: .5,
						y: .5
					},
					x: bounds.centerXPx / cw,
					y: (bounds.centerYPx - inkShiftPx) / ch
				},
				fontSize,
				rotation: bounds.rotationDeg
			};
			if (oldWidthPx > 0) {
				const rawW = currentProps.width;
				updates.width = rawW && typeof rawW === "object" && rawW.unit === "px" ? {
					value: Math.max(1, Math.round(bounds.widthPx)),
					unit: "px"
				} : Math.max(.001, cw > 0 ? bounds.widthPx / cw : .001);
			}
			return updates;
		}
	},
	props: {
		text: {
			default: "Hello World",
			description: "The text to display (single line)",
			ui: {
				type: "text",
				label: "Text",
				group: "Text",
				hidden: true
			}
		},
		fontFamily: {
			default: "Inter",
			description: "Google Fonts family name",
			ui: {
				type: "font-family",
				label: "Font",
				group: "Text"
			}
		},
		fontWeight: {
			default: 400,
			description: "Font weight (only weights published for the family render true; others are synthesized)",
			ui: {
				type: "font-weight",
				label: "Weight",
				group: "Text"
			}
		},
		italic: {
			default: false,
			description: "Italic style",
			ui: {
				type: "checkbox",
				label: "Italic",
				group: "Text"
			},
			transform: (value) => value ? 1 : 0
		},
		textTransform: {
			default: "none",
			description: "Case transformation applied to the text",
			ui: {
				type: "select",
				options: [
					{
						label: "None",
						value: "none"
					},
					{
						label: "Uppercase",
						value: "uppercase"
					},
					{
						label: "Lowercase",
						value: "lowercase"
					},
					{
						label: "Capitalize",
						value: "capitalize"
					}
				],
				label: "Transform",
				group: "Text"
			}
		},
		fontSize: {
			default: .07,
			description: "Font size as a fraction of canvas height",
			ui: {
				type: "range",
				min: .01,
				max: .5,
				step: .005,
				label: "Size",
				group: "Typography",
				dimensional: "canvas-height"
			}
		},
		letterSpacing: {
			default: 0,
			description: "Letter spacing in em units",
			ui: {
				type: "range",
				min: -.1,
				max: .5,
				step: .005,
				label: "Letter Spacing",
				group: "Typography"
			}
		},
		lineHeight: {
			default: 1.2,
			description: "Line height as a multiple of font size (multi-line text)",
			ui: {
				type: "range",
				min: .8,
				max: 2.5,
				step: .05,
				label: "Line Height",
				group: "Paragraph",
				hidden: true
			}
		},
		textAlign: {
			default: "center",
			description: "Horizontal alignment of lines within the text block",
			ui: {
				type: "select",
				options: [
					{
						label: "Left",
						value: "left"
					},
					{
						label: "Center",
						value: "center"
					},
					{
						label: "Right",
						value: "right"
					}
				],
				label: "Align",
				group: "Paragraph",
				hidden: true
			}
		},
		width: {
			default: 0,
			description: "Wrap width — text wraps to fit; 0 = auto (hug contents, single line per explicit break). Edited via the Typography panel's Auto/Fixed width toggle and the side drag handles, not a slider.",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .005,
				label: "Max Width",
				group: "Paragraph",
				units: ["%", "px"],
				dimensional: "canvas-width",
				hidden: true
			}
		},
		color: {
			default: "#ffffff",
			transform: transformColor,
			description: "Text color",
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the text relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the text",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		rotation: {
			default: 0,
			description: "Rotation in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		}
	},
	gpu: { fragment: (params) => {
		const glyphTex = createGlyphRasterHost(params);
		const frame = glyphFrame(params);
		return tintGlyph(glyphTap(glyphTex.kit, frame), frame, params.uniforms.color);
	} }
});
var Text_default = componentDefinition;
export { wrapLines as a, measureTextBlock as i, componentDefinition as n, measureText as r, Text_default as t };
