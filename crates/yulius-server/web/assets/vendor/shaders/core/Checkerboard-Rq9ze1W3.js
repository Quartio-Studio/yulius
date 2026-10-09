import { Ac as defineStd, Ml as p, mo as strokeOver, qa as checkerCells } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Checkerboard",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Classic checkerboard pattern with two alternating colors",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#cccccc",
			transform: transformColor,
			description: "First color of the checkerboard pattern",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#999999",
			transform: transformColor,
			description: "Second color of the checkerboard pattern",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		cells: {
			default: 8,
			description: "Number of cells along the canvas height (creates square cells). Switch the unit to px in the editor to set an absolute cell size instead.",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 50,
				step: 1,
				label: "Cells",
				group: "Effect",
				dimensional: "count-canvas-height"
			}
		},
		softness: {
			default: 0,
			description: "Smoothness of the transition between colors (0 = hard edges, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Softness",
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
		fill: p("colorA"),
		stroke: p("colorB"),
		mask: checkerCells({
			cells: p("cells"),
			softness: p("softness")
		}),
		space: p("colorSpace")
	})
});
var Checkerboard_default = componentDefinition;
export { componentDefinition as n, Checkerboard_default as t };
