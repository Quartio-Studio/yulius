import { Ac as defineStd, Ka as cellFrame, Ml as p, yo as weaveThreads } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Weave",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Interlaced textile weave pattern with two thread colors going over and under each other",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#c4c4c4",
			transform: transformColor,
			description: "Horizontal thread color",
			ui: {
				type: "color",
				label: "X Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#4d4d4d",
			transform: transformColor,
			description: "Vertical thread color",
			ui: {
				type: "color",
				label: "Y Color",
				group: "Colors"
			}
		},
		cells: {
			default: 10,
			description: "Number of threads across the shortest canvas edge",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 40,
				step: 1,
				label: "Cells",
				group: "Effect"
			}
		},
		gap: {
			default: .25,
			description: "Gap between threads (0 = no gap, 0.5 = maximum gap)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .45,
				step: .01,
				label: "Gap",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the weave pattern in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		}
	},
	paint: weaveThreads({
		frame: cellFrame({
			cells: p("cells"),
			rotation: p("rotation"),
			convention: "plain"
		}),
		gap: p("gap"),
		colors: [p("colorA"), p("colorB")]
	})
});
var Weave_default = componentDefinition;
export { componentDefinition as n, Weave_default as t };
