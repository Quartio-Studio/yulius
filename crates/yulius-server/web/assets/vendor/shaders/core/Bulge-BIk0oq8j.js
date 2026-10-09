import { Ac as defineStd, Bn as bulge, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Bulge",
	role: "warp",
	category: "Distortions",
	description: "Magnify or pinch content around a center point",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The center point of the bulge effect"),
		strength: {
			default: 1,
			description: "The intensity of the bulge effect (positive = bulge out, negative = pinch in)",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		radius: {
			default: 1,
			description: "The radius of the bulge effect area",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		falloff: {
			default: .5,
			description: "Controls the smoothness of the transition (0 = hard edge, 1 = very smooth)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Falloff",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("stretch", "How to handle edges when distortion pushes content out of bounds")
	},
	map: bulge({
		center: p("center"),
		strength: p("strength"),
		radius: p("radius"),
		falloff: p("falloff")
	}),
	missingChildMessage: "You must pass a child component into the Bulge shader."
});
var Bulge_default = componentDefinition;
export { componentDefinition as n, Bulge_default as t };
