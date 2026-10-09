import { Ac as defineStd, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig, Xn as mirrorLine } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Mirror",
	role: "warp",
	category: "Distortions",
	description: "Mirror content across a line defined by center point and angle",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The point the mirror line passes through"),
		angle: {
			default: 0,
			description: "The angle of the mirror line in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("mirror", "How to handle edges when distortion pushes content out of bounds")
	},
	map: mirrorLine({
		center: p("center"),
		angle: p("angle")
	}),
	missingChildMessage: "You must pass a child component into the Mirror shader."
});
var Mirror_default = componentDefinition;
export { componentDefinition as n, Mirror_default as t };
