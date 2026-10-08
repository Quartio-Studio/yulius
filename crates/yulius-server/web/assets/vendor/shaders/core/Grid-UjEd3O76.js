import { Ac as defineStd, Ka as cellFrame, Ml as p, do as sampleMapsAtCellCentres, eo as gridLines, mo as strokeOver, vo as vary } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var field = gridLines({
	frame: cellFrame({
		cells: p("cells"),
		rotation: p("rotation"),
		convention: "flippedY"
	}),
	thickness: p("thickness"),
	softness: p("softness"),
	variation: p("variation")
});
const componentDefinition = defineStd({
	name: "Grid",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Simple grid lines pattern with adjustable thickness and rotation",
	acceptsUVContext: true,
	props: {
		color: {
			default: "#ffffff",
			transform: transformColor,
			description: "The color of the grid lines",
			ui: {
				type: "color",
				label: "Line Color",
				group: "Colors"
			}
		},
		cellColor: {
			default: "transparent",
			transform: transformColor,
			description: "Fill color of the cells (transparent = lines only). Pair with Variation for a tiled look.",
			ui: {
				type: "color",
				label: "Cell Color",
				group: "Colors"
			}
		},
		cells: {
			default: 10,
			description: "Number of cells along the canvas height (cells stay square; the width fits as many as the aspect ratio allows)",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 50,
				step: 1,
				label: "Cells",
				group: "Effect"
			}
		},
		thickness: {
			default: 1,
			description: "Thickness of grid lines (normalized, 0.0-1.0)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "Thickness",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the grid in degrees. At 45° this produces a crosshatch/diamond pattern.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Softness of the line edges (0 = crisp, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		variation: {
			default: 0,
			description: "Per-cell random lightening/darkening of the cell fill color",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Variation",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	mapSampleUVs: sampleMapsAtCellCentres({
		cells: p("cells"),
		rotation: p("rotation"),
		props: [
			"thickness",
			"cells",
			"rotation",
			"softness"
		]
	}),
	paint: strokeOver({
		fill: vary(p("cellColor"), field.shade),
		stroke: p("color"),
		mask: field.lines,
		space: p("colorSpace")
	})
});
var Grid_default = componentDefinition;
export { componentDefinition as n, Grid_default as t };
