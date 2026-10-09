import { Ac as defineStd, Ka as cellFrame, Ml as p, mo as strokeOver, to as hexLines, vo as vary } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var field = hexLines({
	frame: cellFrame({
		cells: p("cells"),
		rotation: p("rotation"),
		convention: "clockwise"
	}),
	thickness: p("thickness"),
	softness: p("softness"),
	variation: p("variation")
});
const componentDefinition = defineStd({
	name: "HexGrid",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Honeycomb hexagonal grid pattern",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#000000",
			transform: transformColor,
			description: "Cell fill color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Grid line color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		cells: {
			default: 8,
			description: "Number of hexagons along the canvas height (the width fits as many as the aspect ratio allows)",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 40,
				step: 1,
				label: "Cells",
				group: "Effect"
			}
		},
		thickness: {
			default: 1,
			description: "Thickness of the hex grid lines",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Thickness",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			transform: transformAngle,
			description: "Rotation of the grid in degrees",
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
	paint: strokeOver({
		fill: vary(p("colorA"), field.shade),
		stroke: p("colorB"),
		mask: field.lines,
		space: p("colorSpace")
	})
});
var HexGrid_default = componentDefinition;
export { componentDefinition as n, HexGrid_default as t };
