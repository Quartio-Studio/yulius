import { Ac as defineStd, Bd as ZERO } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Group",
	role: "structural",
	species: "custom",
	category: "Utilities",
	description: "Container for organizing and composing child effects — supports flex-like flow layout (column/row stacking via the flow prop)",
	requiresChild: true,
	boundingBoxDeclaration: {
		aspectRatio: null,
		boxResamplesContent: true
	},
	props: {},
	gpu: { fragment: ({ childNode }) => {
		return childNode ?? ZERO;
	} }
});
var Group_default = componentDefinition;
export { componentDefinition as n, Group_default as t };
