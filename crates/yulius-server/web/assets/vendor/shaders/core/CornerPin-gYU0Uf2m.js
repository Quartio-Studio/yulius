import { Ac as defineStd, Hn as cornerPin, Ml as p, Pl as edgesPropConfig } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "CornerPin",
	role: "warp",
	category: "Distortions",
	description: "Pin each corner of the content to an arbitrary position for a free perspective warp",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		topLeft: {
			default: {
				x: 0,
				y: 0
			},
			transform: transformPosition,
			description: "Position of the top-left corner",
			ui: {
				type: "position",
				label: "Top Left",
				group: "Corners"
			}
		},
		topRight: {
			default: {
				x: 1,
				y: 0
			},
			transform: transformPosition,
			description: "Position of the top-right corner",
			ui: {
				type: "position",
				label: "Top Right",
				group: "Corners"
			}
		},
		bottomLeft: {
			default: {
				x: 0,
				y: 1
			},
			transform: transformPosition,
			description: "Position of the bottom-left corner",
			ui: {
				type: "position",
				label: "Bottom Left",
				group: "Corners"
			}
		},
		bottomRight: {
			default: {
				x: 1,
				y: 1
			},
			transform: transformPosition,
			description: "Position of the bottom-right corner",
			ui: {
				type: "position",
				label: "Bottom Right",
				group: "Corners"
			}
		},
		amount: {
			default: 1,
			description: "Blends the warp in and out — 0 returns the content to its original rectangle, 1 fully pins the corners",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Amount",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("transparent", "How to handle areas outside the pinned quad")
	},
	map: cornerPin({
		topLeft: p("topLeft"),
		topRight: p("topRight"),
		bottomRight: p("bottomRight"),
		bottomLeft: p("bottomLeft"),
		amount: p("amount")
	})
});
var CornerPin_default = componentDefinition;
export { componentDefinition as n, CornerPin_default as t };
