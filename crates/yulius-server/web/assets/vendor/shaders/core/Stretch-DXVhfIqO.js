import { $n as stretch, Ac as defineStd, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Stretch",
	role: "warp",
	category: "Distortions",
	description: "Stretch content towards a direction from a center point",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The center point of the stretch effect"),
		strength: {
			default: 1,
			description: "The intensity of the stretch effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			description: "The direction of the stretch in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		falloff: {
			default: 0,
			description: "Controls the sharpness of the transition (0 = sharp edge, 1 = gradual transition)",
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
	map: stretch({
		center: p("center"),
		strength: p("strength"),
		angle: p("angle"),
		falloff: p("falloff")
	}),
	missingChildMessage: "You must pass a child component into the Stretch shader."
});
var Stretch_default = componentDefinition;
export { componentDefinition as n, Stretch_default as t };
