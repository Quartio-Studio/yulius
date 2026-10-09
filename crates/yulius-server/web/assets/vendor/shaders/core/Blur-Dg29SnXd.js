import { Ac as defineStd, Ml as p, cn as gaussianBlur } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Blur",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "A simple Gaussian blur effect",
	requiresRTT: true,
	requiresChild: true,
	props: { intensity: {
		default: 50,
		description: "Intensity of the blur effect",
		ui: {
			type: ["range", "map"],
			min: 0,
			max: 200,
			step: 1,
			label: "Intensity",
			group: "Effect"
		}
	} },
	...gaussianBlur({ intensity: p("intensity") })
});
var Blur_default = componentDefinition;
export { componentDefinition as n, Blur_default as t };
