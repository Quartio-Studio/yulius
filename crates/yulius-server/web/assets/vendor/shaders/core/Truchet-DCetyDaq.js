import { Ac as defineStd, Ka as cellFrame, Ml as p, _o as truchetArcs, mo as strokeOver } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Truchet",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Quarter-circle arc tiles that connect to form organic, maze-like flowing curves",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#000000",
			transform: transformColor,
			description: "Background color between the arcs",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Arc line color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		cells: {
			default: 10,
			description: "Number of tiles across the shortest canvas edge",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 40,
				step: 1,
				label: "Cells",
				group: "Effect"
			}
		},
		thickness: {
			default: 2,
			description: "Thickness of the arc lines",
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
			transform: transformAngle,
			description: "Rotation of the tiling in degrees",
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
			description: "Softness of the arc edges (0 = crisp, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Random seed — changes which tiles flip, producing a different maze pattern",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
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
		mask: truchetArcs({
			frame: cellFrame({
				cells: p("cells"),
				rotation: p("rotation"),
				convention: "clockwise"
			}),
			thickness: p("thickness"),
			softness: p("softness"),
			seed: p("seed")
		}),
		space: p("colorSpace")
	})
});
var Truchet_default = componentDefinition;
export { componentDefinition as n, Truchet_default as t };
