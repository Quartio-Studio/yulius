import { Ac as defineStd, Kt as blurPath, Ml as p, fn as motionBlur } from "./std-ClftfWpF.js";
import { r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "LinearBlur",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Directional motion blur in a specific angle",
	props: {
		intensity: {
			default: 30,
			description: "Intensity of the linear blur effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Direction of the linear blur (in degrees)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		}
	},
	effect: motionBlur({
		path: blurPath.linear(p("angle")),
		amount: p("intensity")
	})
});
var LinearBlur_default = componentDefinition;
export { componentDefinition as n, LinearBlur_default as t };
