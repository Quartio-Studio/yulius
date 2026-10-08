import { Ac as defineStd, Ml as p, q as sobelTaps, z as chalkSketch } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Chalkboard",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Renders content as a chalk drawing on a blackboard, with edge strokes and cross-hatch shading",
	props: {
		boardColor: {
			default: "#000000",
			transform: transformColor,
			description: "Background board color",
			ui: {
				type: "color",
				label: "Board",
				group: "Colors"
			}
		},
		chalkColor: {
			default: "#eceadb",
			transform: transformColor,
			description: "Chalk stroke color",
			ui: {
				type: "color",
				label: "Chalk",
				group: "Colors"
			}
		},
		edgeSensitivity: {
			default: .5,
			description: "How readily edges are detected and drawn as chalk strokes",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Sensitivity",
				group: "Effect"
			}
		},
		edgeThickness: {
			default: 1.5,
			description: "Thickness of the chalk outline strokes",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 4,
				step: .1,
				label: "Edge Thickness",
				group: "Effect"
			}
		},
		shading: {
			default: .05,
			description: "Amount of cross-hatch shading filling darker regions",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shading",
				group: "Effect"
			}
		},
		hatchScale: {
			default: 16,
			description: "Spacing of the cross-hatch lines in pixels",
			ui: {
				type: ["range", "map"],
				min: 3,
				max: 24,
				step: .5,
				label: "Hatch Scale",
				group: "Effect"
			}
		},
		grain: {
			default: .45,
			description: "Chalk dustiness / grain that breaks up the strokes",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Grain",
				group: "Effect"
			}
		}
	},
	effect: chalkSketch({
		edges: sobelTaps({ spacing: p("edgeThickness") }),
		sensitivity: p("edgeSensitivity"),
		hatchScale: p("hatchScale"),
		shading: p("shading"),
		grain: p("grain"),
		board: p("boardColor"),
		chalk: p("chalkColor")
	})
});
var Chalkboard_default = componentDefinition;
export { componentDefinition as n, Chalkboard_default as t };
