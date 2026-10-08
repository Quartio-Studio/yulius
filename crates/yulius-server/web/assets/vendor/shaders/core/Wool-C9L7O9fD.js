import { Ac as defineStd, Gi as noiseRelief, Zi as reliefBases, c as reliefStylizeProps } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Wool",
	role: "filter",
	species: "custom",
	requiresRTT: true,
	requiresChild: true,
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Applies an interwoven fibrous fabric texture and distortion to child content",
	props: reliefStylizeProps({
		intensity: .5,
		scale: 4,
		contrast: -.5,
		distortion: .15
	}),
	gpu: { fragment: noiseRelief(reliefBases.wool) }
});
var Wool_default = componentDefinition;
export { componentDefinition as n, Wool_default as t };
