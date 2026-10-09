import { Ac as defineStd, Jn as kaleidoscope, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Kaleidoscope",
	role: "warp",
	category: "Distortions",
	description: "Create a kaleidoscope effect with radial mirrored segments",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The center point of the kaleidoscope effect"),
		segments: {
			default: 6,
			description: "Number of radial segments in the kaleidoscope",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 24,
				step: 1,
				label: "Segments",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			description: "Rotation offset for the entire kaleidoscope pattern",
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
	map: kaleidoscope({
		center: p("center"),
		segments: p("segments"),
		angle: p("angle")
	}),
	missingChildMessage: "You must pass a child component into the Kaleidoscope shader."
});
var Kaleidoscope_default = componentDefinition;
export { componentDefinition as n, Kaleidoscope_default as t };
