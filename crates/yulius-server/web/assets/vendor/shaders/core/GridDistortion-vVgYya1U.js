import { Ac as defineStd, Cs as recompileWhen, Ml as p, Pl as edgesPropConfig, Q as pointerSplatField, Z as clampSplatGridSize, et as gridSim, qn as gridCellDisplace } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "GridDistortion",
	role: "warp",
	category: "Interactive",
	description: "Interactive grid distortion controlled by mouse position",
	boundingBoxDeclaration: { aspectRatio: null },
	usesPointer: true,
	props: {
		intensity: {
			default: 1,
			description: "Strength of the distortion effect",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		decay: {
			default: 3,
			description: "Rate of distortion decay (higher = faster)",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .1,
				label: "Decay",
				group: "Effect"
			}
		},
		radius: {
			default: 1,
			description: "Radius of the distortion effect",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		gridSize: {
			default: 20,
			recompile: recompileWhen((prev, next) => clampSplatGridSize(prev) !== clampSplatGridSize(next)),
			description: "Resolution of the distortion grid (higher = more detailed)",
			ui: {
				type: ["range", "map"],
				min: 8,
				max: 128,
				step: 1,
				label: "Grid Size",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("stretch", "How to handle edges when distortion pushes content out of bounds")
	},
	...gridSim(pointerSplatField({
		gridSize: p("gridSize"),
		decay: p("decay"),
		intensity: p("intensity"),
		radius: p("radius"),
		output: "displacement"
	})),
	map: gridCellDisplace({
		gridSize: p("gridSize"),
		output: "displacement"
	}),
	uvRemapIdentityWhen: ({ computeOutputs }) => !computeOutputs?.displacement
});
var GridDistortion_default = componentDefinition;
export { componentDefinition as n, GridDistortion_default as t };
