import { Ac as defineStd, Kn as fromPolar, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "RectangularCoordinates",
	role: "warp",
	category: "Distortions",
	description: "Convert polar coordinates back to rectangular space",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("The center point for rectangular coordinate conversion"),
		scale: {
			default: 1,
			description: "Scale factor for the rectangular output",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 3,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		intensity: {
			default: 1,
			description: "Blends between original UVs (0) and rectangular coordinates (1)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("transparent", "How to handle edges when distortion pushes content out of bounds")
	},
	map: fromPolar({
		center: p("center"),
		scale: p("scale"),
		intensity: p("intensity")
	}),
	missingChildMessage: "You must pass a child component into the RectangularCoordinates shader."
});
var RectangularCoordinates_default = componentDefinition;
export { componentDefinition as n, RectangularCoordinates_default as t };
