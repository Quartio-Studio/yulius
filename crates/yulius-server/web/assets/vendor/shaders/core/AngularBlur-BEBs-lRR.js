import { Ac as defineStd, Kt as blurPath, Ml as p, fn as motionBlur } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "AngularBlur",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Radial motion blur rotating around a center point",
	props: {
		intensity: {
			default: 20,
			description: "Intensity of the angular blur effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Blur Intensity",
				group: "Effect"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the rotation",
			ui: {
				type: "position",
				label: "Center Position",
				group: "Position"
			}
		}
	},
	effect: motionBlur({
		path: blurPath.orbit(p("center")),
		amount: p("intensity")
	})
});
var AngularBlur_default = componentDefinition;
export { componentDefinition as n, AngularBlur_default as t };
