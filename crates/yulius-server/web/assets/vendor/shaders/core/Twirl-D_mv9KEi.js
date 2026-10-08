import { Ac as defineStd, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig, tr as twirl } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Twirl",
	role: "warp",
	category: "Distortions",
	description: "Rotate and twist content around a center point",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The center point of the twirl effect"),
		intensity: {
			default: 1,
			description: "The strength of the twirl effect",
			ui: {
				type: ["range", "map"],
				min: -5,
				max: 5,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("stretch", "How to handle edges when distortion pushes content out of bounds")
	},
	map: twirl({
		center: p("center"),
		intensity: p("intensity")
	}),
	missingChildMessage: "You must pass a child component into the Twirl shader."
});
var Twirl_default = componentDefinition;
export { componentDefinition as n, Twirl_default as t };
