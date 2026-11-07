/**
 * Links to a data model and to one type of it, for every page that names either (T-2766; DM-61,
 * DM-62, UI-85). A name the person cannot open (the models list failed, or no model of the
 * project carries the type) stays plain text: a link is offered only to what the person may read.
 */
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import type { Manifest } from "../../api/manifest";
import { entityTypesOf, spaceOf } from "../spaces/spaceFacts";
import { useProjectList } from "./ModelsList";

const LINK = "focus-ring text-primary-soft-fg underline-offset-2 hover:underline";

/** The model that carries `type`: the one of `space` when one is named and carries it, else the first. */
export function modelOfType(models: Manifest[] | undefined, type: string, space?: string): Manifest | undefined {
  const carrying = (models ?? []).filter((model) => entityTypesOf(model).includes(type));
  return (space === undefined ? undefined : carrying.find((model) => spaceOf(model) === space)) ?? carrying[0];
}

/** A model's name as a link to its page, when the project has a model of that name. */
export function ModelLink({
  project,
  name,
  className = "",
  children,
}: {
  project: string;
  name: string;
  className?: string;
  children?: string;
}): JSX.Element {
  const models = useProjectList(project, "datamodels");
  const known = models.data?.some((model) => model.metadata.name === name) ?? false;
  if (!known) {
    return <span className={className}>{children ?? name}</span>;
  }
  return (
    <Link to="/projects/$project/models/$name" params={{ project, name }} className={`${LINK} ${className}`}>
      {children ?? name}
    </Link>
  );
}

/** An entity type as a link that opens its model on that class, when a model carries it. */
export function TypeLink({
  project,
  type,
  space,
  className = "",
}: {
  project: string;
  type: string;
  /** The space the type is read in, so the space's own model wins when two carry the type. */
  space?: string;
  className?: string;
}): JSX.Element {
  const models = useProjectList(project, "datamodels");
  const model = modelOfType(models.data, type, space);
  if (model === undefined) {
    return <span className={className}>{type}</span>;
  }
  return (
    <Link
      to="/projects/$project/models/$name"
      params={{ project, name: model.metadata.name }}
      search={{ class: type }}
      className={`${LINK} ${className}`}
    >
      {type}
    </Link>
  );
}
