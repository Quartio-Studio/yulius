import { $f as f32, Ac as defineStd, B as charGrid, H as glyphTint, Ml as p, R as cellSample, V as glyphFor, _d as createFontDependentRaster, gd as createCanvasRasterTarget, vd as createRasterInvalidator, yd as createSwappableMediaTexture } from "./std-ClftfWpF.js";
import { i as transformBoolean } from "./transformations-P50Uo_Ju.js";
var ATLAS_TEXTURE_SIZE = 2048;
var GOOGLE_FONTS_MAP = {
	"JetBrains Mono": "JetBrains+Mono",
	"Fira Code": "Fira+Code",
	"Source Code Pro": "Source+Code+Pro",
	"IBM Plex Mono": "IBM+Plex+Mono",
	"Space Mono": "Space+Mono",
	"Roboto Mono": "Roboto+Mono",
	"Courier Prime": "Courier+Prime",
	"Geist Mono": "Geist+Mono",
	"VT323": "VT323",
	"Press Start 2P": "Press+Start+2P",
	"Silkscreen": "Silkscreen",
	"Major Mono Display": "Major+Mono+Display",
	"Syne Mono": "Syne+Mono",
	"Nova Mono": "Nova+Mono",
	"Xanh Mono": "Xanh+Mono",
	"Cutive Mono": "Cutive+Mono",
	"Share Tech Mono": "Share+Tech+Mono",
	"Martian Mono": "Martian+Mono",
	"Azeret Mono": "Azeret+Mono"
};
var loadedFonts = /* @__PURE__ */ new Set();
async function loadAsciiFont(fontFamily) {
	if (loadedFonts.has(fontFamily) || !GOOGLE_FONTS_MAP[fontFamily]) return;
	const googleFontName = GOOGLE_FONTS_MAP[fontFamily];
	if (!document.querySelector(`link[href*="${googleFontName}"]`)) {
		const link = document.createElement("link");
		link.rel = "stylesheet";
		link.href = `https://fonts.googleapis.com/css2?family=${googleFontName}:wght@400&display=swap`;
		document.head.appendChild(link);
	}
	try {
		if (document.fonts && document.fonts.load) {
			for (const spec of [
				`400 12px "${fontFamily}"`,
				`12px "${fontFamily}"`,
				`400 12px ${fontFamily}`,
				`12px ${fontFamily}`
			]) try {
				await document.fonts.load(spec);
				break;
			} catch {}
			await new Promise((r) => setTimeout(r, 500));
		} else await new Promise((r) => setTimeout(r, 1e3));
		loadedFonts.add(fontFamily);
	} catch {}
}
var num = (v, f) => typeof v === "number" ? v : f;
var setupGlyphAtlas = (params) => {
	const { getCpuValue, setExtraField } = params;
	const glyphTex = createSwappableMediaTexture(params, {
		label: "Ascii:atlas",
		initial: {
			width: ATLAS_TEXTURE_SIZE,
			height: ATLAS_TEXTURE_SIZE
		}
	});
	const target = createCanvasRasterTarget(params, { contextAttributes: { willReadFrequently: true } });
	const buildAtlas = (characters, spacing, fontFamily) => {
		if (glyphTex.disposed || !characters || characters.length === 0) return;
		const charCount = characters.length;
		const atlasSize = Math.max(2, Math.ceil(Math.sqrt(charCount)));
		const baseAtlasCellSize = 128;
		const spacingMultiplier = Math.max(1, 2 / spacing);
		const actualCellSize = Math.min(baseAtlasCellSize * spacingMultiplier, ATLAS_TEXTURE_SIZE / atlasSize);
		const fontSize = actualCellSize * .75;
		const c = target.canvas() ? target.context() : target.resize(ATLAS_TEXTURE_SIZE, ATLAS_TEXTURE_SIZE);
		if (!c) return;
		c.clearRect(0, 0, ATLAS_TEXTURE_SIZE, ATLAS_TEXTURE_SIZE);
		c.fillStyle = "#ffffff";
		c.font = `${fontSize}px "${fontFamily}", ${fontFamily}, monospace`;
		if (!c.font.includes(fontFamily) && !c.font.includes("IBM") && !c.font.includes("Plex")) c.font = `${fontSize}px ${fontFamily}, monospace`;
		c.textAlign = "center";
		c.textBaseline = "middle";
		for (let i = 0; i < charCount; i++) {
			const row = Math.floor(i / atlasSize);
			const col = i % atlasSize;
			c.fillText(characters[i], col * actualCellSize + actualCellSize / 2, row * actualCellSize + actualCellSize / 2);
		}
		glyphTex.write(target.canvas());
		const uvScale = atlasSize * actualCellSize / ATLAS_TEXTURE_SIZE;
		setExtraField("_charCount", charCount);
		setExtraField("_atlasScale", uvScale);
		setExtraField("_atlasSize", atlasSize);
	};
	const readProps = () => ({
		characters: String(getCpuValue("characters") ?? componentDefinition.props.characters.default),
		fontFamily: String(getCpuValue("fontFamily") ?? componentDefinition.props.fontFamily.default),
		spacing: num(getCpuValue("spacing"), 1.5)
	});
	const rebuild = () => {
		const p$1 = readProps();
		buildAtlas(p$1.characters, p$1.spacing, p$1.fontFamily);
	};
	const font = createFontDependentRaster({
		key: () => readProps().fontFamily,
		load: () => loadAsciiFont(readProps().fontFamily),
		onLoaded: rebuild
	});
	setTimeout(() => {
		rebuild();
		font.ensure();
	}, 0);
	createRasterInvalidator(params, {
		key: () => {
			const p$1 = readProps();
			return `${p$1.characters}|${p$1.fontFamily}|${p$1.spacing}`;
		},
		run: () => {
			rebuild();
			font.ensure();
		}
	});
	return glyphTex.kit;
};
var grid = charGrid({
	cellSize: p("cellSize"),
	spacing: p("spacing")
});
var cell = cellSample({ grid });
const componentDefinition = defineStd({
	name: "Ascii",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Convert imagery to ASCII character art",
	requiresRTT: true,
	requiresChild: true,
	extraFields: {
		_charCount: {
			schema: f32,
			initial: 9
		},
		_atlasScale: {
			schema: f32,
			initial: 1
		},
		_atlasSize: {
			schema: f32,
			initial: 3
		}
	},
	props: {
		characters: {
			default: "@%#*+=-:.",
			description: "Characters ordered from dense to sparse. First character is used for bright areas, last for dark areas.",
			ui: {
				type: "text",
				label: "Characters",
				group: "Effect"
			}
		},
		cellSize: {
			default: 30,
			description: "Size of each ASCII character cell (normalized to 1080p reference, scales proportionally at other resolutions)",
			ui: {
				type: "range",
				min: 8,
				max: 100,
				step: 1,
				label: "Cell Size",
				group: "Effect"
			}
		},
		fontFamily: {
			default: "JetBrains Mono",
			description: "Font family for characters",
			ui: {
				type: "select",
				options: [
					{
						label: "Azeret Mono",
						value: "Azeret Mono"
					},
					{
						label: "Courier Prime",
						value: "Courier Prime"
					},
					{
						label: "Cutive Mono",
						value: "Cutive Mono"
					},
					{
						label: "Fira Code",
						value: "Fira Code"
					},
					{
						label: "Geist Mono",
						value: "Geist Mono"
					},
					{
						label: "IBM Plex Mono",
						value: "IBM Plex Mono"
					},
					{
						label: "JetBrains Mono",
						value: "JetBrains Mono"
					},
					{
						label: "Major Mono Display",
						value: "Major Mono Display"
					},
					{
						label: "Martian Mono",
						value: "Martian Mono"
					},
					{
						label: "Nova Mono",
						value: "Nova Mono"
					},
					{
						label: "Press Start 2P",
						value: "Press Start 2P"
					},
					{
						label: "Roboto Mono",
						value: "Roboto Mono"
					},
					{
						label: "Share Tech Mono",
						value: "Share Tech Mono"
					},
					{
						label: "Silkscreen",
						value: "Silkscreen"
					},
					{
						label: "Source Code Pro",
						value: "Source Code Pro"
					},
					{
						label: "Space Mono",
						value: "Space Mono"
					},
					{
						label: "Syne Mono",
						value: "Syne Mono"
					},
					{
						label: "VT323",
						value: "VT323"
					},
					{
						label: "Xanh Mono",
						value: "Xanh Mono"
					}
				],
				label: "Font Family",
				group: "Font"
			}
		},
		spacing: {
			default: 1,
			description: "Character size within each cell (1.0 = optimal size, 0.0 = smallest)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Character Size",
				group: "Effect"
			},
			transform: (value) => .1 + value * 1.4
		},
		gamma: {
			default: 1,
			description: "Brightness curve adjustment. <1 brightens darks (more light characters), >1 darkens midtones (more dark characters). Use to better fit characters to image brightness range.",
			ui: {
				type: ["range", "map"],
				min: .25,
				max: 3,
				step: .01,
				label: "Gamma",
				group: "Effect"
			}
		},
		alphaThreshold: {
			default: 0,
			description: "Pixels with alpha below this threshold become fully transparent.",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Alpha Threshold",
				group: "Effect"
			}
		},
		preserveAlpha: {
			default: true,
			description: "When enabled, output alpha matches input alpha. When disabled, pixels above the alpha threshold become fully opaque.",
			ui: {
				type: "checkbox",
				label: "Preserve Alpha",
				group: "Effect"
			},
			transform: transformBoolean
		}
	},
	gpu: { fragment: glyphTint({
		grid,
		cell,
		glyph: glyphFor({
			grid,
			cell,
			gamma: p("gamma"),
			atlas: setupGlyphAtlas
		}),
		alphaThreshold: p("alphaThreshold"),
		preserveAlpha: p("preserveAlpha")
	}) }
});
var Ascii_default = componentDefinition;
export { componentDefinition as n, Ascii_default as t };
