import { $ as springLatticeField, Ac as defineStd, Ml as p, Pl as edgesPropConfig, Yn as liquidDisplace, et as gridSim } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Liquify",
	role: "warp",
	category: "Interactive",
	description: "Liquid-like interactive deformation effect",
	boundingBoxDeclaration: { aspectRatio: null },
	usesPointer: true,
	props: {
		intensity: {
			default: 10,
			description: "Scale of the fabric displacement effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		stiffness: {
			default: 3,
			description: "Fabric rigidity (higher = stiffer canvas, lower = stretchy silk)",
			ui: {
				type: "range",
				min: 1,
				max: 30,
				step: .5,
				label: "Stiffness",
				group: "Effect"
			}
		},
		damping: {
			default: 3,
			description: "How quickly fabric motion settles",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .1,
				label: "Damping",
				group: "Effect"
			}
		},
		radius: {
			default: 1,
			description: "Cursor influence area",
			ui: {
				type: "range",
				min: .1,
				max: 1.5,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("stretch", "How to handle edges when distortion pushes content out of bounds")
	},
	...gridSim(springLatticeField({
		stiffness: p("stiffness"),
		damping: p("damping"),
		radius: p("radius"),
		output: "displacement"
	})),
	map: liquidDisplace({
		intensity: p("intensity"),
		output: "displacement"
	}),
	uvRemapIdentityWhen: ({ computeOutputs }) => !computeOutputs?.displacement
});
var Liquify_default = componentDefinition;
export { componentDefinition as n, Liquify_default as t };
