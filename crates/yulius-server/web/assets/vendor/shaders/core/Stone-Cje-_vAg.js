import { Ac as defineStd, Gi as noiseRelief, Zi as reliefBases, c as reliefStylizeProps } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Stone",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Applies a marbled stone relief and surface distortion to child content",
	requiresRTT: true,
	requiresChild: true,
	props: reliefStylizeProps({
		intensity: .5,
		scale: 1,
		contrast: 0,
		distortion: .15
	}),
	gpu: { fragment: noiseRelief(reliefBases.stone) }
});
var Stone_default = componentDefinition;
export { componentDefinition as n, Stone_default as t };
