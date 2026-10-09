import { Ac as defineStd, Dt as brightnessContrast, Ml as p, Ss as isZero, vs as allOf, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "BrightnessContrast",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Adjust brightness and contrast of the image",
	props: {
		brightness: {
			default: 0,
			description: "Brightness adjustment (-1 to 1)",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Brightness",
				group: "Adjustments"
			}
		},
		contrast: {
			default: 0,
			description: "Contrast adjustment (-1 to 1)",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Contrast",
				group: "Adjustments"
			}
		}
	},
	effect: brightnessContrast({
		brightness: p("brightness"),
		contrast: p("contrast")
	}),
	identityWhen: allOf(isZero("brightness"), isZero("contrast")),
	missingChildMessage: "You must pass a child component into the Brightness Contrast shader."
});
var BrightnessContrast_default = componentDefinition;
export { componentDefinition as n, BrightnessContrast_default as t };
